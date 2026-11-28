//! The importer: GitHub folder in, `doc` entries out.
//!
//! Written against the [`Env`] trait so the whole algorithm runs in
//! ordinary unit tests; the plugin's WebAssembly entry points supply an
//! `Env` backed by the host imports.
//!
//! A run is idempotent and resumable. Every write carries a hash of what
//! it would store, and the host skips a write whose hash it already holds,
//! so a run that ran out of budget is finished by the next one, and a run
//! with nothing new costs one conditional request.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use crate::blocks::{self, Links};
use crate::frontmatter;
use crate::github::{self, Repo};
use crate::plan::{self, Kind, Meta, PageInfo, Planned, Root};

/// Bump to rewrite every entry after an importer change. 0.1.9 bumps it (f3)
/// once so that the first sync after the upgrade writes every entry again: a
/// host that now repairs a wrong stored status (a draft root) then does so
/// for entries whose source did not change.
pub const FORMAT: &str = "f3";
/// Largest page the importer reads.
pub const MAX_PAGE_BYTES: u64 = 512 * 1024;
/// Page text kept between the scan and the write of one run.
const MAX_KEPT_BODIES: usize = 8 * 1024 * 1024;
/// Most documents one sync may list, over the main tree and every section
/// together. 2,000 stopped a site with three sections of 650 pages plus its
/// main tree (2,672) before any write; each run is limited by its request and
/// write budgets anyway, so the cap only guards against a wrong folder.
pub const MAX_PAGES: usize = 5000;
/// Key prefix of the commit an unfinished sync works on, per ref: its hash
/// and GitHub's etag for it, one value, so a 304 names the right commit.
const WORKING_PREFIX: &str = "sync/working/";
/// Key prefix of the packed scan cache. The broker allows 600 key-value
/// reads a minute even in an extended task, so a site of a thousand pages
/// cannot keep one key per page: the cache is a few packs of ~40 KB.
const PACK_PREFIX: &str = "scanpack/";
/// Cached tree listings, keyed by commit and folder.
const TREE_PREFIX: &str = "treecache/";
/// Largest serialized pack (the key-value store refuses 64 KiB).
const PACK_MAX_BYTES: usize = 40_000;
/// Largest single cached text; a bigger one is read again next time.
const PACK_MAX_ENTRY: usize = 20_000;
/// Old one-key-per-page cache entries removed per finished sync.
const LEGACY_SCAN_DELETES: usize = 100;

/// Blob sha to cached scan (or `_meta.json` text), loaded in a few reads.
struct ScanCache {
    map: BTreeMap<String, String>,
    /// Pack keys that existed when loaded.
    packs: Vec<String>,
    dirty: bool,
}

impl ScanCache {
    /// Reads every pack. A refused read stops the load: a cache that looked
    /// empty because the host said "not now" made a throttled sync fetch
    /// every page again, and never finish.
    fn load<E: Env>(env: &mut E) -> Result<Self, String> {
        let mut packs = env.kv_list(PACK_PREFIX)?;
        packs.sort();
        let mut map = BTreeMap::new();
        for k in &packs {
            let parsed = env
                .kv_get(k)?
                .and_then(|v| serde_json::from_str::<BTreeMap<String, String>>(&v).ok());
            if let Some(m) = parsed {
                map.extend(m);
            }
        }
        Ok(Self {
            map,
            packs,
            dirty: false,
        })
    }

    fn insert(&mut self, sha: &str, text: &str) {
        if text.len() <= PACK_MAX_ENTRY && self.map.get(sha).map(String::as_str) != Some(text) {
            self.map.insert(sha.to_owned(), text.to_owned());
            self.dirty = true;
        }
    }

    fn retain_live(&mut self, live: &BTreeSet<&str>) {
        let before = self.map.len();
        self.map.retain(|k, _| live.contains(k.as_str()));
        if self.map.len() != before {
            self.dirty = true;
        }
    }

    /// Writes the packs back (a handful of writes); best effort, a full
    /// quota only makes the next run read some pages again.
    fn flush<E: Env>(&mut self, env: &mut E) {
        if !self.dirty {
            return;
        }
        let mut chunks: Vec<BTreeMap<&str, &str>> = vec![BTreeMap::new()];
        let mut size = 2usize;
        for (k, v) in &self.map {
            let cost = k.len() + v.len() * 2 + 8;
            if size + cost > PACK_MAX_BYTES && !chunks.last().is_some_and(BTreeMap::is_empty) {
                chunks.push(BTreeMap::new());
                size = 2;
            }
            size += cost;
            if let Some(c) = chunks.last_mut() {
                c.insert(k, v);
            }
        }
        let mut keys = Vec::new();
        for (i, c) in chunks.iter().enumerate() {
            if c.is_empty() {
                continue;
            }
            let key = format!("{PACK_PREFIX}{i:03}");
            let json = serde_json::to_string(c).unwrap_or_default();
            if env.kv_set(&key, &json).is_err() {
                return;
            }
            keys.push(key);
        }
        for old in &self.packs {
            if !keys.contains(old) {
                env.kv_delete(old);
            }
        }
        self.packs = keys;
        self.dirty = false;
    }
}

/// The post type written.
pub const POST_TYPE: &str = "doc";
/// The URL base of the type.
pub const BASE: &str = "docs";
/// The version taxonomy.
pub const VERSION_TAXONOMY: &str = "doc-version";
/// The section taxonomy.
pub const SECTION_TAXONOMY: &str = "doc-section";
/// A section's pseudo-version starts with this; the rest is its slug.
const SECTION_PREFIX: &str = "section/";

/// What an operator configured.
#[derive(Debug, Clone)]
pub struct Config {
    /// The repository.
    pub repo: Repo,
    /// Branch.
    pub branch: String,
    /// Docs folder, as path segments (empty for the repository root).
    pub folder: Vec<String>,
    /// Access token for a private repository.
    pub token: Option<String>,
    /// The version served at `/docs` itself; empty for unversioned docs.
    pub latest_version: String,
    /// Older versions: label and the folder they live in.
    pub versions: Vec<(String, Vec<String>)>,
    /// Versions read from another branch or tag than `branch`: label to ref.
    pub version_refs: BTreeMap<String, String>,
    /// Versions that document unreleased code: their pages stay out of search engines.
    pub unreleased: Vec<String>,
    /// Doc sections: own folder (and ref), served below `/docs/<slug>`.
    pub sections: Vec<crate::settings::SectionDef>,
}

/// An HTTP answer, reduced to what the importer reads.
#[derive(Debug, Clone)]
pub struct Reply {
    /// Status code.
    pub status: u16,
    /// `ETag`, when present.
    pub etag: Option<String>,
    /// Response text.
    pub body: String,
}

/// An entry to write (mirrors the host's `entry-input`).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Stable key.
    pub key: String,
    /// Parent's key.
    pub parent_key: Option<String>,
    /// Slug.
    pub slug: String,
    /// Title.
    pub title: String,
    /// Excerpt.
    pub excerpt: String,
    /// Blocks, as JSON text.
    pub blocks_json: String,
    /// Sibling order.
    pub position: i32,
    /// `taxonomy:slug` terms.
    pub terms: Vec<String>,
    /// Field values, as JSON text.
    pub fields_json: String,
    /// SEO overrides, as JSON text.
    pub seo_json: String,
    /// Hash of everything above plus the importer format.
    pub source_hash: String,
    /// "Edit on GitHub" address.
    pub edit_url: String,
}

/// What the host did with an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    /// New.
    Created,
    /// Changed.
    Updated,
    /// Same hash: nothing written.
    Unchanged,
}

/// An entry the plugin owns, as the host lists it.
#[derive(Debug, Clone)]
pub struct Managed {
    /// Key.
    pub key: String,
    /// Stored hash.
    pub source_hash: String,
}

/// The world outside the algorithm.
pub trait Env {
    /// HTTPS GET.
    fn get(&mut self, url: &str, headers: &[(&str, String)]) -> Result<Reply, String>;
    /// Milliseconds since the epoch.
    fn now_ms(&self) -> u64;
    /// Reads a stored value.
    ///
    /// # Errors
    /// The host's message; [`QUOTA_EXCEEDED`] when this minute's read
    /// budget is spent (the run pauses and the next one carries on).
    fn kv_get(&mut self, key: &str) -> Result<Option<String>, String>;
    /// Stores a value.
    ///
    /// # Errors
    /// Quota or size.
    fn kv_set(&mut self, key: &str, value: &str) -> Result<(), String>;
    /// Removes a value.
    fn kv_delete(&mut self, key: &str);
    /// Keys under a prefix.
    ///
    /// # Errors
    /// As [`Env::kv_get`].
    fn kv_list(&mut self, prefix: &str) -> Result<Vec<String>, String>;
    /// Writes an entry.
    ///
    /// # Errors
    /// The host's message.
    fn put_entry(&mut self, entry: &Entry) -> Result<Put, String>;
    /// Entries this plugin owns.
    ///
    /// # Errors
    /// The host's message.
    fn list_managed(&mut self) -> Result<Vec<Managed>, String>;
    /// Trashes an entry.
    ///
    /// # Errors
    /// The host's message.
    fn delete_entry(&mut self, key: &str) -> Result<(), String>;
}

/// How much one run may do.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Absolute stop time, ms since the epoch.
    pub deadline_ms: u64,
    /// Most HTTP requests.
    pub max_requests: u32,
    /// Most entry writes that change something.
    pub max_writes: u32,
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Entries created.
    pub created: u32,
    /// Entries changed.
    pub updated: u32,
    /// Entries left alone.
    pub unchanged: u32,
    /// Entries trashed.
    pub deleted: u32,
    /// Requests made.
    pub requests: u32,
    /// Everything was applied (false: run again).
    pub finished: bool,
    /// The host refused a read or write for its per-minute quota; the run
    /// stopped where it was and the next one carries on.
    pub throttled: bool,
    /// Nothing changed upstream since the last finished run.
    pub up_to_date: bool,
    /// The commit synced.
    pub commit: String,
    /// Notes for the operator.
    pub warnings: Vec<String>,
    /// Pages the host refused, as "path: what happened (why)": imported in a
    /// simpler form or left out. Kept between runs until the page is written
    /// as it is or leaves the repository. `None` when the run stopped before
    /// it looked at the pages (an up-to-date run reads nothing): the last
    /// list still holds.
    pub problems: Option<Vec<String>>,
    /// More pages were refused than the list holds.
    pub more_problems: bool,
}

impl Report {
    fn count(&mut self, put: Put) {
        match put {
            Put::Created => self.created += 1,
            Put::Updated => self.updated += 1,
            Put::Unchanged => self.unchanged += 1,
        }
    }
}

/// Where the pages the host refused are listed between runs.
pub const PROBLEMS_KEY: &str = "sync/problems";
/// Most refused pages listed.
const MAX_PROBLEMS: usize = 100;
/// Largest serialized list: well under the store's 64 KiB, so the list and
/// the status that repeats its texts are always saved.
const MAX_PROBLEMS_BYTES: usize = 48 * 1024;
/// Longest entry key the list keeps as it is (the host's own key limit).
const MAX_LISTED_KEY_BYTES: usize = 512;
/// Longest path and reason kept in a listed text.
const MAX_TEXT_PART_BYTES: usize = 300;

/// The first `max` bytes of `s`, cut on a character boundary.
fn cut_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The key a page is listed under: its entry key, or for a key the host
/// refuses for its length, its start and a hash of the whole.
fn listed_key(key: &str) -> String {
    if key.len() <= MAX_LISTED_KEY_BYTES {
        return key.to_owned();
    }
    let digest = hex(&Sha256::digest(key.as_bytes()));
    format!("{}#{}", cut_bytes(key, 400), &digest[..16])
}

/// Pages the host refused, by entry key: kept in one key-value entry.
struct Problems {
    map: BTreeMap<String, String>,
    /// A refused page was not listed: the list is full.
    full: bool,
    dirty: bool,
}

impl Problems {
    fn load<E: Env>(env: &mut E) -> Result<Self, String> {
        let v: serde_json::Value = env
            .kv_get(PROBLEMS_KEY)?
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        // Stored as {"pages": {key: text}, "full": bool}; before 0.1.16 the
        // map alone.
        let (pages, full) = if v["pages"].is_object() {
            (v["pages"].clone(), v["full"].as_bool().unwrap_or(false))
        } else {
            (v, false)
        };
        Ok(Self {
            map: serde_json::from_value(pages).unwrap_or_default(),
            full,
            dirty: false,
        })
    }

    fn serialized(&self) -> String {
        serde_json::json!({"pages": self.map, "full": self.full}).to_string()
    }

    /// Lists a page as "`name`: `how` (`reason`)", the parts cut so that the
    /// list stays within its byte limit; a page that does not fit is counted
    /// as "more".
    fn set(&mut self, key: &str, name: &str, how: &str, reason: &str) {
        let key = listed_key(key);
        let text = format!(
            "{}: {how} ({})",
            cut_bytes(name, MAX_TEXT_PART_BYTES),
            cut_bytes(reason, MAX_TEXT_PART_BYTES)
        );
        if self.map.get(&key) == Some(&text) {
            return;
        }
        let new = !self.map.contains_key(&key);
        if new && self.map.len() >= MAX_PROBLEMS {
            self.dirty |= !self.full;
            self.full = true;
            return;
        }
        let old = self.map.insert(key.clone(), text);
        if self.serialized().len() > MAX_PROBLEMS_BYTES {
            match old {
                Some(o) => {
                    self.map.insert(key, o);
                }
                None => {
                    self.map.remove(&key);
                }
            }
            self.dirty |= !self.full;
            self.full = true;
            return;
        }
        self.dirty = true;
    }

    fn contains(&self, key: &str) -> bool {
        self.map.contains_key(&listed_key(key))
    }

    fn clear(&mut self, key: &str) {
        self.dirty |= self.map.remove(&listed_key(key)).is_some();
        if self.map.is_empty() {
            self.dirty |= self.full;
            self.full = false;
        }
    }

    /// Forgets pages that are no longer in the plan. "More" stays until the
    /// list is empty; a page in its simpler form that was left off is listed
    /// again by a later run once there is room.
    fn retain(&mut self, live: &BTreeSet<&str>) {
        let live: BTreeSet<String> = live.iter().map(|k| listed_key(k)).collect();
        let before = self.map.len();
        self.map.retain(|k, _| live.contains(k));
        self.dirty |= self.map.len() != before;
        if self.map.is_empty() {
            self.dirty |= self.full;
            self.full = false;
        }
    }

    /// Saves a changed list.
    fn save<E: Env>(&mut self, env: &mut E) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if self.map.is_empty() && !self.full {
            env.kv_delete(PROBLEMS_KEY);
        } else {
            env.kv_set(PROBLEMS_KEY, &self.serialized())?;
        }
        self.dirty = false;
        Ok(())
    }

    fn lines(&self) -> Vec<String> {
        self.map.values().cloned().collect()
    }
}

/// Where the entries the host refused outright are remembered, so a
/// continuation run does not fetch and offer them again.
pub const REFUSED_KEY: &str = "sync/refused";
/// Most refused entries remembered (16 hex characters each).
const MAX_REFUSED: usize = 2_500;

/// Entries the host refused even in their simpler form, each as a digest of
/// its key, its source hash and the commits synced: a new commit, plan or
/// FORMAT tries them again, and so does "Sync now", which forgets them.
struct Refused {
    before: BTreeSet<String>,
    now: BTreeSet<String>,
}

impl Refused {
    fn load<E: Env>(env: &mut E) -> Result<Self, String> {
        let before = env
            .kv_get(REFUSED_KEY)?
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Ok(Self {
            before,
            now: BTreeSet::new(),
        })
    }

    fn digest(key: &str, hash: &str, commits: &str) -> String {
        let d = hex(&Sha256::digest(
            format!("{key}\n{hash}\n{commits}").as_bytes(),
        ));
        d[..16].to_owned()
    }

    /// Whether this entry was refused at this version; it is kept if so.
    fn check(&mut self, digest: &str) -> bool {
        let known = self.before.contains(digest) || self.now.contains(digest);
        if known {
            self.note(digest);
        }
        known
    }

    /// Past [`MAX_REFUSED`] the rest are not remembered and are offered
    /// again on every run: only a site with more pages than that refused
    /// outright (not under a parent the site lacks) pays for it.
    fn note(&mut self, digest: &str) {
        if self.now.len() < MAX_REFUSED {
            self.now.insert(digest.to_owned());
        }
    }

    /// Saves the entries refused this run; with `all_seen` (the whole plan was
    /// looked at) the older ones are dropped. A lost write only means one
    /// more try of each page.
    fn save<E: Env>(&self, env: &mut E, all_seen: bool) {
        let mut keep = self.now.clone();
        if !all_seen {
            for d in &self.before {
                if keep.len() >= MAX_REFUSED {
                    break;
                }
                keep.insert(d.clone());
            }
        }
        if keep == self.before {
            return;
        }
        if keep.is_empty() {
            env.kv_delete(REFUSED_KEY);
        } else if let Ok(t) = serde_json::to_string(&keep) {
            let _ = env.kv_set(REFUSED_KEY, &t);
        }
    }
}

/// The host refused this entry for what it holds (its blocks, title or key,
/// or a parent it could not write), not for a quota or a storage fault: the
/// entry is imported another way or left out, and the sync goes on.
fn is_refusal(e: &str) -> bool {
    e.starts_with("validation failed")
        || e.starts_with("blocks-json")
        || e.starts_with("title must be")
        || e.starts_with("key must be")
        || (e.starts_with("parent ") && e.contains("has not been written"))
}

/// The source hash an entry's simpler form is written with: a run that finds
/// it knows the page was refused at this version without reading it again.
fn simpler_hash(hash: &str) -> String {
    hex(&Sha256::digest(format!("plain:{hash}").as_bytes()))
}

/// The page as one block of plain text: its Markdown as written.
fn plain_text_entry(entry: &Entry, markdown: &str) -> Entry {
    let code = markdown.strip_suffix('\n').unwrap_or(markdown);
    Entry {
        blocks_json: serde_json::json!([
            {"kind": "code", "attrs": {"code": code, "language": null}, "children": []}
        ])
        .to_string(),
        ..entry.clone()
    }
}

struct Ctx<'a, E: Env> {
    env: &'a mut E,
    cfg: &'a Config,
    budget: Budget,
    report: Report,
}

enum Stop {
    Budget,
    /// The host's per-minute quota: a pause, like [`Stop::Budget`].
    Throttled,
    Fail(String),
}

/// The host's error for a read or write over its per-minute quota.
pub const QUOTA_EXCEEDED: &str = "quota-exceeded";

impl From<String> for Stop {
    fn from(s: String) -> Self {
        if s.starts_with(QUOTA_EXCEEDED) {
            Stop::Throttled
        } else {
            Stop::Fail(s)
        }
    }
}

impl<E: Env> Ctx<'_, E> {
    fn get(&mut self, url: &str, accept: &str, etag: Option<&str>) -> Result<Reply, Stop> {
        if self.report.requests >= self.budget.max_requests
            || self.env.now_ms() >= self.budget.deadline_ms
        {
            return Err(Stop::Budget);
        }
        self.report.requests += 1;
        let raw = url.starts_with("https://raw.githubusercontent.com/");
        let mut headers: Vec<(&str, String)> = vec![("accept", accept.to_owned())];
        if !raw {
            headers.push(("x-github-api-version", "2022-11-28".to_owned()));
        }
        if let Some(t) = &self.cfg.token {
            headers.push((
                "authorization",
                if raw {
                    format!("token {t}")
                } else {
                    format!("Bearer {t}")
                },
            ));
        }
        if let Some(e) = etag {
            headers.push(("if-none-match", e.to_owned()));
        }
        let reply = self.env.get(url, &headers)?;
        match reply.status {
            200 | 304 => Ok(reply),
            401 => Err(Stop::Fail(
                "GitHub refused the access token (401)".to_owned(),
            )),
            403 | 429 => Err(Stop::Fail(
                "GitHub rate limit or permission problem (403); an access token raises the limit"
                    .to_owned(),
            )),
            404 => Err(Stop::Fail(
                "repository, branch or folder not found, or the token cannot see it (404)"
                    .to_owned(),
            )),
            s => Err(Stop::Fail(format!("GitHub answered {s}"))),
        }
    }
}

/// Runs one sync.
///
/// # Errors
/// A message for the operator when the repository cannot be read; running
/// out of budget is not an error (`finished` is false).
pub fn run<E: Env>(
    env: &mut E,
    cfg: &Config,
    hint_commit: Option<&str>,
    budget: Budget,
) -> Result<Report, String> {
    let mut cx = Ctx {
        env,
        cfg,
        budget,
        report: Report::default(),
    };
    match sync(&mut cx, hint_commit) {
        Ok(()) => {
            cx.report.finished = true;
            Ok(cx.report)
        }
        Err(Stop::Budget) => Ok(cx.report),
        Err(Stop::Throttled) => {
            cx.report.throttled = true;
            Ok(cx.report)
        }
        Err(Stop::Fail(m)) => Err(m),
    }
}

fn sync<E: Env>(cx: &mut Ctx<E>, hint: Option<&str>) -> Result<(), Stop> {
    // 1. Which commits? One conditional request per branch or tag in use
    // (the main one, plus any a version is read from): unchanged costs nothing.
    let mut refs: Vec<String> = vec![cx.cfg.branch.clone()];
    for r in cx
        .cfg
        .version_refs
        .values()
        .chain(cx.cfg.sections.iter().filter_map(|s| s.git_ref.as_ref()))
    {
        if !refs.contains(r) {
            refs.push(r.clone());
        }
    }
    // Done means done by this importer: after an upgrade that
    // changes what pages are made of, the same commit syncs again.
    let done = cx.env.kv_get("sync/complete")?.as_deref() == Some("1")
        && cx.env.kv_get("sync/format")?.as_deref() == Some(FORMAT);
    let mut heads: Vec<String> = Vec::new();
    let mut all_unchanged = true;
    for (i, r) in refs.iter().enumerate() {
        let (etag_key, head_key) = if i == 0 {
            ("sync/commit-etag".to_owned(), "sync/head/0".to_owned())
        } else {
            (format!("sync/commit-etag/{i}"), format!("sync/head/{i}"))
        };
        let sha = match hint {
            Some(h) if i == 0 && h.len() >= 40 => {
                all_unchanged = false;
                h.to_owned()
            }
            _ => {
                let url = format!(
                    "https://api.github.com/repos/{}/{}/commits/{}",
                    cx.cfg.repo.owner,
                    cx.cfg.repo.name,
                    github::encode_path(r)
                );
                // A finished sync asks with the etag of the commit it
                // finished. An unfinished one asks with the etag of the
                // commit it is working on, kept beside that commit in one
                // value: without a token GitHub allows 60 API calls an hour,
                // and a 304 does not count, while asking for every head in
                // full each minute used them all within half an hour, so a
                // long import failed with 403 before it could finish.
                let (kept, etag) = if done {
                    (cx.env.kv_get(&head_key)?, cx.env.kv_get(&etag_key)?)
                } else {
                    match cx.env.kv_get(&format!("{WORKING_PREFIX}{i}"))? {
                        Some(v) => match v.split_once('\n') {
                            Some((sha, e)) => (Some(sha.to_owned()), Some(e.to_owned())),
                            None => (None, None),
                        },
                        None => (None, None),
                    }
                };
                let use_etag = kept.is_some() && etag.is_some();
                let reply = cx.get(
                    &url,
                    "application/vnd.github.sha",
                    if use_etag { etag.as_deref() } else { None },
                )?;
                if reply.status == 304 && use_etag {
                    kept.unwrap_or_default()
                } else {
                    all_unchanged = false;
                    let sha = reply.body.trim().to_owned();
                    if let Some(e) = &reply.etag {
                        let _ = cx.env.kv_set(&format!("{etag_key}-next"), e);
                        let _ = cx
                            .env
                            .kv_set(&format!("{WORKING_PREFIX}{i}"), &format!("{sha}\n{e}"));
                    }
                    sha
                }
            }
        };
        if sha.len() < 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Stop::Fail("GitHub did not return a commit hash".to_owned()));
        }
        heads.push(sha);
    }
    // Unchanged heads mean nothing to do only once the sync of them finished;
    // an unfinished one carries on with them.
    if all_unchanged && done {
        cx.report.up_to_date = true;
        return Ok(());
    }
    let commit = heads[0].clone();
    cx.report.commit.clone_from(&commit);
    let combined = heads.join(",");
    // A new commit is marked unfinished before anything else is written:
    // a write refused part way (a quiet pause now) must not leave
    // `complete = "1"` beside a new commit, or the next tick would take
    // the half-done sync for a finished one and wait `poll_minutes`.
    let new_commit = cx.env.kv_get("sync/commit")?.as_deref() != Some(combined.as_str());
    if new_commit {
        cx.env.kv_set("sync/complete", "0")?;
    }
    for (i, sha) in heads.iter().enumerate() {
        cx.env.kv_set(&format!("sync/head/{i}"), sha)?;
    }
    if new_commit {
        cx.env.kv_set("sync/commit", &combined)?;
    }
    let commit_of = |r: &str| -> String {
        refs.iter()
            .position(|x| x == r)
            .map_or_else(|| commit.clone(), |i| heads[i].clone())
    };

    // 2. Files of every tree.
    let mut trees: Vec<TreeSpec> = Vec::new();
    if lists_main_tree(cx.cfg) {
        trees.push(TreeSpec {
            label: cx.cfg.latest_version.clone(),
            folder: cx.cfg.folder.clone(),
            tag: "main".to_owned(),
            git_ref: cx.cfg.branch.clone(),
            commit: commit.clone(),
        });
    }
    for (label, folder) in &cx.cfg.versions {
        let git_ref = cx
            .cfg
            .version_refs
            .get(label)
            .cloned()
            .unwrap_or_else(|| cx.cfg.branch.clone());
        trees.push(TreeSpec {
            label: label.clone(),
            folder: folder.clone(),
            tag: format!("v:{label}"),
            git_ref: git_ref.clone(),
            commit: commit_of(&git_ref),
        });
    }
    for sec in &cx.cfg.sections {
        let git_ref = sec.git_ref.clone().unwrap_or_else(|| cx.cfg.branch.clone());
        trees.push(TreeSpec {
            label: sec.slug.clone(),
            folder: sec.folder.clone(),
            tag: format!("s:{}", sec.slug),
            commit: commit_of(&git_ref),
            git_ref,
        });
    }
    let mut files: Vec<SrcFile> = Vec::new();
    for t in &trees {
        let listing = list_folder(cx, &t.commit, &t.folder)?;
        for e in github::doc_files(&listing, MAX_PAGE_BYTES) {
            files.push(SrcFile {
                tree: t.clone(),
                path: e.path.clone(),
                sha: e.sha.clone(),
            });
        }
    }
    if files.len() > MAX_PAGES {
        return Err(Stop::Fail(format!(
            "{} documents found; the importer reads at most {MAX_PAGES}",
            files.len()
        )));
    }

    // 3. Phase A: read every page's front matter and title (cached by blob).
    let mut infos: BTreeMap<String, Scan> = BTreeMap::new();
    // Bodies read in this phase, kept for the next one so a page is
    // fetched once per run (up to a memory cap; past it, it is fetched again).
    let mut bodies: BTreeMap<String, String> = BTreeMap::new();
    let mut bodies_bytes = 0usize;
    let mut metas: BTreeMap<(String, String), Meta> = BTreeMap::new();
    // The cache is read once (a few packs), kept in memory, and written back
    // when this phase ends, however it ends, so a run cut short by the
    // request budget resumes where it stopped.
    let mut cache = ScanCache::load(cx.env)?;
    let scanned: Result<(), Stop> = (|| {
        for f in &files {
            let is_meta = f.path == "_meta.json" || f.path.ends_with("/_meta.json");
            let text = if let Some(c) = cache.map.get(&f.sha) {
                c.clone()
            } else {
                let url = github::raw_url(
                    &cx.cfg.repo,
                    &f.tree.commit,
                    &join_repo_path(&f.tree.folder, &f.path),
                );
                let body = cx.get(&url, "text/plain", None)?.body;
                if is_meta {
                    // `_meta.json` is cached as it is; a text too big for a
                    // pack is simply read again next time.
                    cache.insert(&f.sha, &body);
                    body
                } else {
                    let json = scan_page(&body).to_json();
                    cache.insert(&f.sha, &json);
                    if bodies_bytes + body.len() <= MAX_KEPT_BODIES {
                        bodies_bytes += body.len();
                        bodies.insert(f.sha.clone(), body);
                    }
                    json
                }
            };
            if is_meta {
                let dir = f.path.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
                metas.insert((f.tree.tag.clone(), dir), plan::parse_meta(&text));
            } else {
                infos.insert(
                    file_key(f),
                    Scan::from_json(&text).ok_or_else(|| {
                        Stop::Fail(format!("cache entry for {} is unreadable", f.path))
                    })?,
                );
            }
        }
        Ok(())
    })();
    cache.flush(cx.env);
    scanned?;

    // 4. Plan.
    let p = make_plan(cx.cfg, &files, &infos, &metas);
    cx.report.warnings.extend(p.warnings.clone());
    let sig = plan_signature(&p.entries);

    // 5. Phase B: write what changed, parents first.
    let managed: BTreeMap<String, String> = cx
        .env
        .list_managed()?
        .into_iter()
        .map(|m| (m.key, m.source_hash))
        .collect();
    // Trash what no current page or section owns FIRST, so pages from an
    // earlier configuration (before the sections were set) leave the site and
    // its sitemap now, not when a long import finally ends.
    let wanted: BTreeSet<&str> = p.entries.iter().map(|e| e.planned.key.as_str()).collect();
    // Managed rows stay listed after they are trashed, so a cursor (the last
    // key handled) lets a budget-limited run carry on where the last stopped,
    // and the plan signature says the whole set was already done.
    if cx.env.kv_get("sync/pruned")?.as_deref() != Some(sig.as_str()) {
        let cursor = cx.env.kv_get("sync/prune-cursor")?.unwrap_or_default();
        let mut last: Option<String> = None;
        for key in managed.keys().filter(|k| k.as_str() > cursor.as_str()) {
            if wanted.contains(key.as_str()) {
                continue;
            }
            if cx.report.deleted >= cx.budget.max_writes {
                if let Some(last) = &last {
                    cx.env.kv_set("sync/prune-cursor", last)?;
                }
                return Err(Stop::Budget);
            }
            cx.env.delete_entry(key)?;
            cx.report.deleted += 1;
            last = Some(key.clone());
        }
        cx.env.kv_set("sync/pruned", &sig)?;
        cx.env.kv_delete("sync/prune-cursor");
    }
    let today = civil_date(cx.env.now_ms());
    let mut problems = Problems::load(cx.env)?;
    let mut refused = Refused::load(cx.env)?;
    // Entries the site does not have after this run: their children are
    // refused too, so they are listed without being read or offered.
    let mut absent: BTreeSet<String> = BTreeSet::new();
    let wrote = (|| -> Result<(), Stop> {
        for pe in &p.entries {
            let key = &pe.planned.key;
            let hash_basis = match &pe.source {
                Source::Page(f) => format!("{FORMAT}:{}:{sig}", f.sha),
                Source::Generated => {
                    format!("{FORMAT}:{}", generated_basis(pe, &p.entries, &infos))
                }
            };
            let hash = hex(&Sha256::digest(hash_basis.as_bytes()));
            if managed.get(key) == Some(&hash) {
                cx.report.unchanged += 1;
                continue;
            }
            // What the problems list calls it.
            let name = match &pe.source {
                Source::Page(f) => join_repo_path(&f.tree.folder, &f.path),
                Source::Generated => key.clone(),
            };
            let simpler = match pe.source {
                Source::Page(_) => "imported as plain text",
                Source::Generated => "imported as a list of its pages",
            };
            // On the site in its simpler form at this version: unchanged too,
            // and listed again if it fell off a full list.
            let fallback_hash = simpler_hash(&hash);
            if managed.get(key) == Some(&fallback_hash) {
                cx.report.unchanged += 1;
                if !problems.contains(key) {
                    problems.set(key, &name, simpler, "refused at this version");
                }
                continue;
            }
            // An older version may still be on the site.
            let on_site = managed.contains_key(key);
            let left_out = if on_site {
                "not updated"
            } else {
                "not imported"
            };
            if let Some(parent) = pe
                .planned
                .parent_key
                .as_deref()
                .filter(|k| absent.contains(*k))
            {
                if !on_site {
                    absent.insert(key.clone());
                }
                let why = format!("its parent {parent} is not on the site");
                problems.set(key, &name, left_out, &why);
                continue;
            }
            // Refused outright at this version by an earlier run of this
            // sync: not read or offered again.
            let digest = Refused::digest(key, &hash, &combined);
            if refused.check(&digest) {
                if !on_site {
                    absent.insert(key.clone());
                }
                if !problems.contains(key) {
                    problems.set(key, &name, left_out, "refused at this version");
                }
                continue;
            }
            if cx.report.created + cx.report.updated >= cx.budget.max_writes {
                return Err(Stop::Budget);
            }
            // The entry, and what to write instead when the host refuses it.
            let (entry, fallback) = match &pe.source {
                Source::Page(f) => {
                    let url = github::raw_url(&cx.cfg.repo, &f.tree.commit, &name);
                    let body = match bodies.remove(&f.sha) {
                        Some(b) => b,
                        None => cx.get(&url, "text/plain", None)?.body,
                    };
                    let entry = build_page_entry(
                        cx.cfg,
                        &f.tree.commit,
                        f,
                        &body,
                        pe,
                        &p,
                        &hash,
                        &today,
                        &infos,
                    );
                    let fallback = plain_text_entry(&entry, frontmatter::split(&body).body);
                    (entry, Some(fallback))
                }
                Source::Generated => {
                    // A root with its own top-level index page shows that page.
                    let overview = match overview_of(pe, &p) {
                        Some((o, f)) => {
                            let body = match bodies.get(&f.sha) {
                                Some(b) => b.clone(),
                                None => {
                                    let url = github::raw_url(
                                        &cx.cfg.repo,
                                        &f.tree.commit,
                                        &join_repo_path(&f.tree.folder, &f.path),
                                    );
                                    cx.get(&url, "text/plain", None)?.body
                                }
                            };
                            Some(
                                build_page_entry(
                                    cx.cfg,
                                    &f.tree.commit,
                                    f,
                                    &body,
                                    o,
                                    &p,
                                    &hash,
                                    &today,
                                    &infos,
                                )
                                .blocks_json,
                            )
                        }
                        None => None,
                    };
                    // Without the overview it is a list of cards.
                    let fallback = overview
                        .is_some()
                        .then(|| build_generated_entry(pe, &p, &hash, &infos, None));
                    (
                        build_generated_entry(pe, &p, &hash, &infos, overview),
                        fallback,
                    )
                }
            };
            // A page the host refuses is imported in a simpler form, or left
            // out, and listed; it never stops the rest of the sync.
            let refusal = match cx.env.put_entry(&entry) {
                Ok(put) => {
                    cx.report.count(put);
                    problems.clear(key);
                    continue;
                }
                Err(e) if is_refusal(&e) => e,
                Err(e) => return Err(e.into()),
            };
            let Some(fallback) = fallback else {
                refused.note(&digest);
                if !on_site {
                    absent.insert(key.clone());
                }
                problems.set(key, &name, left_out, &refusal);
                continue;
            };
            // Written with a marked hash, so later runs know it is the
            // simpler form and list it again when the list has room. Listed,
            // and saved, before it is written, so a stop right after it
            // still names the reason.
            let fallback = Entry {
                source_hash: fallback_hash,
                ..fallback
            };
            problems.set(key, &name, simpler, &refusal);
            problems.save(cx.env)?;
            match cx.env.put_entry(&fallback) {
                Ok(put) => cx.report.count(put),
                Err(e) if is_refusal(&e) => {
                    refused.note(&digest);
                    if !on_site {
                        absent.insert(key.clone());
                    }
                    problems.set(key, &name, left_out, &refusal);
                }
                Err(e) => return Err(e.into()),
            }
        }
        problems.retain(&wanted);
        Ok(())
    })();
    // The refused entries are remembered only beside a saved list, so a page
    // that is not offered again is always named.
    if problems.save(cx.env).is_ok() {
        refused.save(cx.env, wrote.is_ok());
    }
    cx.report.problems = Some(problems.lines());
    cx.report.more_problems = problems.full;
    wrote?;

    // 7. Done: remember it, and forget what is no longer in the repository.
    // The per-blob scans stay: they are what lets the next run plan without
    // reading a page that has not changed.
    let live: BTreeSet<&str> = files.iter().map(|f| f.sha.as_str()).collect();
    cache.retain_live(&live);
    cache.flush(cx.env);
    // Importers before 0.1.5 kept one key per page; drop those a few at a time.
    for k in cx
        .env
        .kv_list("scan/")?
        .into_iter()
        .take(LEGACY_SCAN_DELETES)
    {
        cx.env.kv_delete(&k);
    }
    for i in 0..refs.len() {
        let key = if i == 0 {
            "sync/commit-etag".to_owned()
        } else {
            format!("sync/commit-etag/{i}")
        };
        if let Some(next) = cx.env.kv_get(&format!("{key}-next"))? {
            cx.env.kv_set(&key, &next)?;
        }
    }
    // The working commits of the unfinished sync: a finished one asks with
    // `sync/commit-etag`, and the next unfinished one starts afresh.
    for k in cx.env.kv_list(WORKING_PREFIX)? {
        cx.env.kv_delete(&k);
    }
    // Listings of commits no longer in use.
    for k in cx.env.kv_list(TREE_PREFIX)? {
        if !heads
            .iter()
            .any(|h| k.starts_with(&format!("{TREE_PREFIX}{h}/")))
        {
            cx.env.kv_delete(&k);
        }
    }
    cx.env.kv_set("sync/complete", "1")?;
    cx.env.kv_set("sync/format", FORMAT)?;
    cx.env
        .kv_set("sync/last-ok-ms", &cx.env.now_ms().to_string())?;
    Ok(())
}

#[derive(Clone)]
struct TreeSpec {
    label: String,
    folder: Vec<String>,
    tag: String,
    /// The branch or tag this tree is read from.
    git_ref: String,
    /// The commit that ref pointed at for this run.
    commit: String,
}

#[derive(Clone)]
struct SrcFile {
    tree: TreeSpec,
    /// Path below the tree's folder.
    path: String,
    sha: String,
}

fn file_key(f: &SrcFile) -> String {
    format!("{}:{}", f.tree.tag, f.path)
}

/// Whether the sync lists the main tree (Folder on Branch). A site made only
/// of sections leaves Folder empty: the main tree would then be the whole
/// repository root, listing every section's pages a second time. Older
/// versions belong to the main tree, so with any configured it is listed.
fn lists_main_tree(cfg: &Config) -> bool {
    !(cfg.folder.is_empty() && !cfg.sections.is_empty() && cfg.versions.is_empty())
}

fn join_repo_path(folder: &[String], path: &str) -> String {
    if folder.is_empty() {
        path.to_owned()
    } else {
        format!("{}/{path}", folder.join("/"))
    }
}

/// What the importer remembers about a page between runs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scan {
    title: String,
    description: String,
    /// The first paragraph as plain text: the card text when there is no description.
    lead: String,
    slug: Option<String>,
    position: Option<i64>,
    hide: bool,
    version: Option<String>,
    section: Option<String>,
    updated: Option<String>,
}

fn scan_page(src: &str) -> Scan {
    let split = frontmatter::split(src);
    let converted = blocks::convert(split.body, &Links::default());
    let lead = converted
        .blocks
        .iter()
        .find(|b| b["kind"] == "paragraph")
        .and_then(|b| b["attrs"]["text"].as_str())
        .map(plain_text)
        .unwrap_or_default();
    let h1 = converted.h1;
    Scan {
        title: split.front.title.clone().or(h1).unwrap_or_default(),
        description: split.front.description.clone().unwrap_or_default(),
        lead,
        slug: split.front.slug.clone(),
        position: split.front.position,
        hide: split.front.hide,
        version: split.front.version.clone(),
        section: split.front.section.clone(),
        updated: split.front.updated.clone(),
    }
}

impl Scan {
    fn to_json(&self) -> String {
        serde_json::json!({
            "title": self.title, "description": self.description, "lead": self.lead, "slug": self.slug, "position": self.position,
            "hide": self.hide, "version": self.version, "section": self.section, "updated": self.updated,
        })
        .to_string()
    }

    fn from_json(s: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(s).ok()?;
        let opt = |k: &str| v[k].as_str().map(str::to_owned);
        Some(Self {
            title: v["title"].as_str()?.to_owned(),
            description: opt("description").unwrap_or_default(),
            lead: opt("lead").unwrap_or_default(),
            slug: opt("slug"),
            position: v["position"].as_i64(),
            hide: v["hide"].as_bool().unwrap_or(false),
            version: opt("version"),
            section: opt("section"),
            updated: opt("updated"),
        })
    }
}

enum Source {
    Page(SrcFile),
    Generated,
}

struct PlannedEntry {
    planned: Planned,
    source: Source,
    version: String,
    kind_version_root: bool,
    kind_section_root: bool,
}

struct FullPlan {
    entries: Vec<PlannedEntry>,
    warnings: Vec<String>,
    /// Page key to site path (links).
    known: BTreeMap<(String, String), String>,
}

fn effective_version(cfg: &Config, f: &SrcFile, scan: &Scan) -> String {
    if let Some(slug) = f.tree.tag.strip_prefix("s:") {
        return format!("{SECTION_PREFIX}{slug}");
    }
    scan.version.clone().unwrap_or_else(|| {
        if f.tree.tag == "main" {
            cfg.latest_version.clone()
        } else {
            f.tree.label.clone()
        }
    })
}

fn make_plan(
    cfg: &Config,
    files: &[SrcFile],
    infos: &BTreeMap<String, Scan>,
    metas: &BTreeMap<(String, String), Meta>,
) -> FullPlan {
    // Group pages by the version they belong to.
    let mut by_version: BTreeMap<String, Vec<(&SrcFile, &Scan)>> = BTreeMap::new();
    for f in files {
        let Some(scan) = infos.get(&file_key(f)) else {
            continue;
        };
        by_version
            .entry(effective_version(cfg, f, scan))
            .or_default()
            .push((f, scan));
    }
    let mut entries: Vec<PlannedEntry> = Vec::new();
    let mut warnings = Vec::new();
    let mut known = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    // The latest tree first, then older versions in the order configured.
    order.push(cfg.latest_version.clone());
    for (label, _) in &cfg.versions {
        order.push(label.clone());
    }
    for v in by_version.keys() {
        if !order.contains(v) {
            order.push(v.clone());
        }
    }
    let mut roots_made = 0;
    for version in order {
        let Some(pages) = by_version.get(&version) else {
            continue;
        };
        let section = version.strip_prefix(SECTION_PREFIX);
        let is_latest = section.is_none() && version == cfg.latest_version;
        let (shown, seg) = match section {
            Some(slug) => (
                cfg.sections
                    .iter()
                    .find(|d| d.slug == slug)
                    .map_or_else(|| slug.to_owned(), |d| d.title.clone()),
                slug.to_owned(),
            ),
            None => (version.clone(), plan::slugify(&version)),
        };
        let root_key = match section {
            Some(slug) => format!("@section/{slug}"),
            None => format!("@version/{version}"),
        };
        // A section slug may be `product/version`: the root hangs under `product`, which is
        // generated when no section of that name is configured.
        // The host stores a slug without dots (`0.2` is `0-2`), so links are written in that
        // form instead of relying on a redirect for each one.
        let seg = seg.replace('.', "-");
        let (seg_parent, seg_leaf) = match seg.split_once('/') {
            Some((p, l)) => (Some(p.to_owned()), l.to_owned()),
            None => (None, seg.clone()),
        };
        let parent_root_key = seg_parent.as_ref().map(|p| format!("@section/{p}"));
        if let (Some(p), Some(pk)) = (&seg_parent, &parent_root_key) {
            let configured = cfg.sections.iter().any(|d| &d.slug == p);
            if !configured && !entries.iter().any(|e| &e.planned.key == pk) {
                roots_made += 1;
                entries.push(PlannedEntry {
                    planned: Planned {
                        key: pk.clone(),
                        parent_key: None,
                        slug: p.clone(),
                        title: p.clone(),
                        position: 1000 + roots_made * 10,
                        hide: true,
                        section: None,
                        kind: Kind::Group,
                        site_path: format!("/{BASE}/{p}"),
                    },
                    source: Source::Generated,
                    version: version.clone(),
                    kind_version_root: false,
                    kind_section_root: true,
                });
            }
        }
        if !is_latest {
            roots_made += 1;
            entries.push(PlannedEntry {
                planned: Planned {
                    key: root_key.clone(),
                    parent_key: parent_root_key.clone(),
                    slug: seg_leaf.clone(),
                    title: shown.clone(),
                    position: 1000 + roots_made * 10,
                    hide: true,
                    section: None,
                    kind: Kind::Group,
                    site_path: format!("/{BASE}/{seg}"),
                },
                source: Source::Generated,
                version: version.clone(),
                kind_version_root: section.is_none(),
                kind_section_root: section.is_some(),
            });
        }
        let infos_for: Vec<PageInfo> = pages
            .iter()
            .map(|(f, s)| PageInfo {
                key: file_key(f),
                path: f.path.clone(),
                title: if s.title.is_empty() {
                    plan::slugify(
                        f.path
                            .rsplit('/')
                            .next()
                            .unwrap_or("page")
                            .trim_end_matches(".md")
                            .trim_end_matches(".mdx"),
                    )
                } else {
                    s.title.clone()
                },
                slug: s.slug.clone(),
                position: s.position,
                hide: s.hide,
                section: s.section.clone(),
            })
            .filter(|p| !p.path.ends_with("_meta.json"))
            .collect();
        // `_meta.json` of the first page's tree applies to its folders.
        let mut vmetas: BTreeMap<String, Meta> = BTreeMap::new();
        for ((tag, dir), m) in metas {
            let tree_matches = pages.iter().any(|(f, _)| &f.tree.tag == tag);
            if tree_matches {
                vmetas.insert(dir.clone(), m.clone());
            }
        }
        let root = Root {
            base: BASE.to_owned(),
            prefix: if is_latest {
                Vec::new()
            } else {
                seg.split('/').map(str::to_owned).collect()
            },
            parent_key: if is_latest {
                None
            } else {
                Some(root_key.clone())
            },
            tree: if is_latest {
                "main".to_owned()
            } else {
                seg.replace('/', "-")
            },
        };
        let (planned, w) = plan::plan_tree(&infos_for, &vmetas, &root);
        warnings.extend(w);
        for pl in planned {
            let source = match pages.iter().find(|(f, _)| file_key(f) == pl.key) {
                Some((f, _)) => Source::Page((*f).clone()),
                None => Source::Generated,
            };
            if let Source::Page(f) = &source {
                known.insert((f.tree.tag.clone(), f.path.clone()), pl.site_path.clone());
            }
            entries.push(PlannedEntry {
                planned: pl,
                source,
                version: version.clone(),
                kind_version_root: false,
                kind_section_root: false,
            });
        }
    }
    FullPlan {
        entries,
        warnings,
        known,
    }
}

fn plan_signature(entries: &[PlannedEntry]) -> String {
    let mut h = Sha256::new();
    for e in entries {
        h.update(e.planned.key.as_bytes());
        h.update([0]);
        h.update(e.planned.site_path.as_bytes());
        h.update([0]);
        h.update(e.planned.title.as_bytes());
        h.update([1]);
    }
    hex(&h.finalize())[..16].to_owned()
}

fn generated_basis(
    pe: &PlannedEntry,
    all: &[PlannedEntry],
    infos: &BTreeMap<String, Scan>,
) -> String {
    let mut s = format!(
        "cards2|{}|{}|{}|{}|{}",
        pe.planned.key,
        pe.planned.slug,
        pe.planned.title,
        pe.planned.position,
        pe.planned.parent_key.as_deref().unwrap_or("")
    );
    for c in all
        .iter()
        .filter(|c| c.planned.parent_key.as_deref() == Some(pe.planned.key.as_str()))
    {
        s.push_str(&format!(
            "|{}={}={}|{}",
            c.planned.site_path,
            c.planned.title,
            card_text(c, infos),
            c.planned.hide
        ));
        if let Source::Page(f) = &c.source {
            if !f.path.contains('/') && plan::is_index(&f.path) {
                s.push_str(&format!("|{}", f.sha));
            }
        }
    }
    s
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn list_folder<E: Env>(
    cx: &mut Ctx<E>,
    commit: &str,
    folder: &[String],
) -> Result<github::Tree, Stop> {
    // A listing is fixed by its commit, so a round cut short by the budget
    // reuses it: an anonymous run has 60 GitHub API calls an hour, and
    // listing again on every round used them all before any page was made.
    if let Some(t) = cached_tree(cx.env, commit, folder)? {
        return Ok(t);
    }
    let mut tree_ref = commit.to_owned();
    for seg in folder {
        let url = github::tree_url(&cx.cfg.repo, &tree_ref, false);
        let t = github::parse_tree(&cx.get(&url, "application/vnd.github+json", None)?.body)?;
        let Some(next) = t.entries.iter().find(|e| e.is_dir && e.path == *seg) else {
            return Err(Stop::Fail(format!(
                "folder {} not found in the repository",
                folder.join("/")
            )));
        };
        tree_ref.clone_from(&next.sha);
    }
    let url = github::tree_url(&cx.cfg.repo, &tree_ref, true);
    let t = github::parse_tree(&cx.get(&url, "application/vnd.github+json", None)?.body)?;
    if t.truncated {
        return Err(Stop::Fail("the folder is too large for GitHub to list in one answer; point the importer at a smaller folder".to_owned()));
    }
    cache_tree(cx.env, commit, folder, &t);
    Ok(t)
}

/// The key of part `part` of a cached listing. The part comes before the
/// folder so that no folder name can make one listing's key another's.
fn tree_key(commit: &str, folder: &[String], part: usize) -> String {
    format!("{TREE_PREFIX}{commit}/{part:03}/{}", folder.join("/"))
}

/// Keeps the files the sync reads (path and blob sha of each document) in
/// parts below the key-value store's 64 KiB, like the scan packs. Directories,
/// images and sizes are left out: a folder of 700 pages listed in full is
/// about 66 KB, which the store refused, so it was listed again every run.
/// Best effort: a full quota only costs the next round its requests.
fn cache_tree<E: Env>(env: &mut E, commit: &str, folder: &[String], t: &github::Tree) {
    let rows: Vec<String> = github::doc_files(t, MAX_PAGE_BYTES)
        .into_iter()
        .map(|e| serde_json::json!([e.path, e.sha]).to_string())
        .collect();
    let mut parts: Vec<Vec<&str>> = vec![Vec::new()];
    // Room for {"sha":...,"parts":NNN,"files":[]}.
    let overhead = t.sha.len() + 64;
    let mut size = overhead;
    for r in &rows {
        if size + r.len() + 1 > PACK_MAX_BYTES && !parts.last().is_some_and(Vec::is_empty) {
            parts.push(Vec::new());
            size = overhead;
        }
        size += r.len() + 1;
        if let Some(p) = parts.last_mut() {
            p.push(r);
        }
    }
    // Part 0 says how many parts there are; it is written last, so a listing
    // whose writes stopped part way is never read as complete.
    for (i, p) in parts.iter().enumerate().rev() {
        let json = format!(
            "{{\"sha\":{},\"parts\":{},\"files\":[{}]}}",
            serde_json::Value::from(t.sha.as_str()),
            parts.len(),
            p.join(",")
        );
        if env.kv_set(&tree_key(commit, folder, i), &json).is_err() {
            return;
        }
    }
}

/// A listing kept by [`cache_tree`], or `None` when any part is missing or
/// unreadable (it is then listed again). A refused read stops the run.
fn cached_tree<E: Env>(
    env: &mut E,
    commit: &str,
    folder: &[String],
) -> Result<Option<github::Tree>, String> {
    let Some(first) = env.kv_get(&tree_key(commit, folder, 0))? else {
        return Ok(None);
    };
    let Some((sha, parts, mut entries)) = parse_tree_part(&first) else {
        return Ok(None);
    };
    for i in 1..parts {
        let Some(v) = env.kv_get(&tree_key(commit, folder, i))? else {
            return Ok(None);
        };
        match parse_tree_part(&v) {
            Some((s, n, more)) if s == sha && n == parts => entries.extend(more),
            _ => return Ok(None),
        }
    }
    Ok(Some(github::Tree {
        sha,
        truncated: false,
        entries,
    }))
}

/// One part of a cached listing: the tree sha, the number of parts and its
/// files (documents only, so no directory and a size that passes the cap).
fn parse_tree_part(v: &str) -> Option<(String, usize, Vec<github::TreeEntry>)> {
    let c: serde_json::Value = serde_json::from_str(v).ok()?;
    let mut entries = Vec::new();
    for row in c.get("files")?.as_array()? {
        let r = row.as_array()?;
        entries.push(github::TreeEntry {
            path: r.first()?.as_str()?.to_owned(),
            is_dir: false,
            sha: r.get(1)?.as_str()?.to_owned(),
            size: 0,
        });
    }
    let parts = usize::try_from(c.get("parts")?.as_u64()?).ok()?;
    if parts == 0 {
        return None;
    }
    Some((c.get("sha")?.as_str()?.to_owned(), parts, entries))
}

/// The field values of an entry. Empty text is left out: the host reads
/// a missing value as "not set", but would refuse an empty URL or date.
fn doc_fields(
    version_root: bool,
    hide: bool,
    section: &str,
    edit_url: &str,
    updated: &str,
) -> String {
    let mut fields = serde_json::Map::new();
    fields.insert("hide_from_nav".to_owned(), hide.into());
    fields.insert("version_root".to_owned(), version_root.into());
    for (key, value) in [
        ("section_label", section),
        ("edit_url", edit_url),
        ("last_updated", updated),
    ] {
        if !value.is_empty() {
            fields.insert(key.to_owned(), value.into());
        }
    }
    serde_json::Value::Object(fields).to_string()
}

fn terms_for(version: &str) -> Vec<String> {
    if let Some(slug) = version.strip_prefix(SECTION_PREFIX) {
        vec![format!("{SECTION_TAXONOMY}:{}", plan::slugify(slug))]
    } else if version.is_empty() {
        Vec::new()
    } else {
        vec![format!("{VERSION_TAXONOMY}:{}", plan::slugify(version))]
    }
}

#[allow(clippy::too_many_arguments)]
fn build_page_entry(
    cfg: &Config,
    commit: &str,
    f: &SrcFile,
    body: &str,
    pe: &PlannedEntry,
    p: &FullPlan,
    hash: &str,
    today: &str,
    infos: &BTreeMap<String, Scan>,
) -> Entry {
    let split = frontmatter::split(body);
    // Links resolve against pages of the same tree.
    let mut known = BTreeMap::new();
    for ((tag, path), site) in &p.known {
        if *tag == f.tree.tag {
            known.insert(path.clone(), site.clone());
        }
    }
    let links = Links {
        folder: f.tree.folder.join("/"),
        dir: f
            .path
            .rsplit_once('/')
            .map_or(String::new(), |(d, _)| d.to_owned()),
        known,
        blob_base: github::blob_base(&cfg.repo, commit),
        raw_base: github::raw_base(&cfg.repo, commit),
    };
    let converted = blocks::convert(split.body, &links);
    let scan = infos.get(&file_key(f));
    let updated = scan
        .and_then(|s| s.updated.clone())
        .unwrap_or_else(|| today.to_owned());
    let edit = github::edit_url(
        &cfg.repo,
        &f.tree.git_ref,
        &join_repo_path(&f.tree.folder, &f.path),
    );
    // An older version's page points search engines at its counterpart.
    let canonical = (pe.version != cfg.latest_version && !pe.version.starts_with(SECTION_PREFIX))
        .then(|| {
            let rel = pe.planned.site_path.splitn(4, '/').nth(3).unwrap_or("");
            let latest = format!("/{BASE}/{rel}");
            p.entries
                .iter()
                .any(|e| e.planned.site_path == latest && e.version == cfg.latest_version)
                .then_some(latest)
        })
        .flatten();
    // A hidden page is reachable by its address but kept out of search
    // and search engines, like it is kept out of the sidebar.
    let mut seo = serde_json::Map::new();
    if let Some(c) = canonical {
        seo.insert("canonical-path".to_owned(), c.into());
    }
    if pe.planned.hide || cfg.unreleased.contains(&pe.version) {
        seo.insert("noindex".to_owned(), true.into());
    }
    let seo = serde_json::Value::Object(seo).to_string();
    let mut blocks = converted.blocks;
    if blocks.is_empty() {
        blocks
            .push(serde_json::json!({"kind": "paragraph", "attrs": {"text": ""}, "children": []}));
    }
    Entry {
        key: pe.planned.key.clone(),
        parent_key: pe.planned.parent_key.clone(),
        slug: pe.planned.slug.clone(),
        title: pe.planned.title.clone(),
        excerpt: scan
            .map(|s| s.description.chars().take(300).collect())
            .unwrap_or_default(),
        blocks_json: serde_json::Value::Array(blocks).to_string(),
        position: pe.planned.position,
        terms: terms_for(&pe.version),
        fields_json: doc_fields(
            false,
            pe.planned.hide,
            pe.planned.section.as_deref().unwrap_or(""),
            &edit,
            &updated,
        ),
        seo_json: seo,
        source_hash: hash.to_owned(),
        edit_url: edit,
    }
}

/// Inline markup removed, entities left escaped, one line: for card text.
fn plain_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    let one = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut t: String = one.chars().take(200).collect();
    if one.chars().count() > 200 {
        t.push('\u{2026}');
    }
    t
}

/// The text a card shows under a child's title: its front-matter
/// description, else its first paragraph. Empty for a generated page.
fn card_text(c: &PlannedEntry, infos: &BTreeMap<String, Scan>) -> String {
    let Source::Page(f) = &c.source else {
        return String::new();
    };
    infos.get(&file_key(f)).map_or(String::new(), |s| {
        if s.description.trim().is_empty() {
            s.lead.clone()
        } else {
            s.description.trim().chars().take(300).collect()
        }
    })
}

/// A version or section root shows the tree's top-level `index.md` / `README.md`
/// as its page, when the tree has one.
fn overview_of<'a>(pe: &PlannedEntry, p: &'a FullPlan) -> Option<(&'a PlannedEntry, &'a SrcFile)> {
    if !(pe.kind_version_root || pe.kind_section_root) {
        return None;
    }
    p.entries.iter().find_map(|c| match &c.source {
        Source::Page(f)
            if c.planned.parent_key.as_deref() == Some(pe.planned.key.as_str())
                && !f.path.contains('/')
                && plan::is_index(&f.path) =>
        {
            Some((c, f))
        }
        _ => None,
    })
}

/// One list item per visible child: the title as a link, then its description.
fn card_blocks(
    pe: &PlannedEntry,
    p: &FullPlan,
    infos: &BTreeMap<String, Scan>,
) -> Vec<serde_json::Value> {
    let items: Vec<serde_json::Value> = p
        .entries
        .iter()
        .filter(|c| {
            c.planned.parent_key.as_deref() == Some(pe.planned.key.as_str()) && !c.planned.hide
        })
        .map(|c| {
            let desc = card_text(c, infos);
            let mut text = format!(
                "<a href=\"{}\"><strong>{}</strong></a>",
                blocks::esc(&c.planned.site_path),
                blocks::esc(&c.planned.title)
            );
            if !desc.is_empty() {
                text.push_str(" \u{2014} ");
                text.push_str(&blocks::esc(&desc));
            }
            serde_json::json!({"kind": "paragraph", "attrs": {"text": text}, "children": []})
        })
        .collect();
    if items.is_empty() {
        return vec![
            serde_json::json!({"kind": "paragraph", "attrs": {"text": ""}, "children": []}),
        ];
    }
    vec![serde_json::json!({"kind": "list", "attrs": {"ordered": false}, "children": items})]
}

/// `overview`: the blocks of the tree's top-level index page, when the root has one.
fn build_generated_entry(
    pe: &PlannedEntry,
    p: &FullPlan,
    hash: &str,
    infos: &BTreeMap<String, Scan>,
    overview: Option<String>,
) -> Entry {
    let cards = overview.is_none();
    let blocks_json =
        overview.unwrap_or_else(|| serde_json::Value::Array(card_blocks(pe, p, infos)).to_string());
    Entry {
        key: pe.planned.key.clone(),
        parent_key: pe.planned.parent_key.clone(),
        slug: pe.planned.slug.clone(),
        title: pe.planned.title.clone(),
        excerpt: String::new(),
        blocks_json,
        position: pe.planned.position,
        terms: terms_for(&pe.version),
        fields_json: {
            let mut f: serde_json::Value = serde_json::from_str(&doc_fields(
                pe.kind_version_root,
                pe.planned.hide,
                "",
                "",
                "",
            ))
            .unwrap_or_default();
            if pe.kind_section_root {
                f["section_root"] = true.into();
            }
            if cards {
                f["cards"] = true.into();
            }
            f.to_string()
        },
        seo_json: "{}".to_owned(),
        source_hash: hash.to_owned(),
        edit_url: String::new(),
    }
}

/// `YYYY-MM-DD` for a time in ms since the epoch (proleptic Gregorian, UTC).
#[must_use]
pub fn civil_date(ms: u64) -> String {
    let days = i64::try_from(ms / 86_400_000).unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// Calls up to this long are ordinary host calls; a sync needs more.
pub const ORDINARY_BUDGET_MS: u64 = 10_000;
/// Kept back for the last request, the parse after it and the status write.
/// One GitHub tree request takes about 300 ms and its listing 160 KB to parse.
pub const RESERVE_MS: u64 = 1_000;

/// Whether a call of `total` ms may run a sync at all.
pub fn sync_fits(total: u64) -> bool {
    total > ORDINARY_BUDGET_MS
}

/// The part of `total` ms in which a new request may start.
pub fn usable_ms(total: u64) -> u64 {
    total.saturating_sub((total / 5).max(RESERVE_MS))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_ordinary_call_does_no_sync_work_and_a_request_keeps_a_one_second_reserve() {
        assert!(!sync_fits(2_000));
        assert!(!sync_fits(10_000));
        assert!(sync_fits(60_000));
        assert_eq!(usable_ms(2_000), 1_000);
        assert_eq!(usable_ms(60_000), 48_000);
        assert_eq!(usable_ms(500), 0);
    }

    use super::*;
    use std::cell::RefCell;

    const SHA1: &str = "1111111111111111111111111111111111111111";
    const SHA2: &str = "2222222222222222222222222222222222222222";

    type Files = BTreeMap<String, String>;

    #[derive(Default)]
    struct Fake {
        commits: BTreeMap<String, Files>, // commit -> files (full repo paths)
        head: String,
        /// Other branches and tags: ref -> commit. Anything else is `head`.
        refs: BTreeMap<String, String>,
        /// The commit the last root listing named; directory listings follow it.
        walking: String,
        kv: RefCell<BTreeMap<String, String>>,
        entries: BTreeMap<String, (Entry, String)>,
        now: u64,
        requests: Vec<String>,
        etag_seen: Vec<Option<String>>,
        fail_status: Option<u16>,
        step_ms: u64,
        /// Block kinds the host refuses, like a host that validates blocks.
        refuse_kinds: Vec<&'static str>,
        /// Entry keys the host refuses whatever their blocks.
        refuse_keys: Vec<String>,
        /// Key-value reads (`kv_get` and `kv_list`): the broker allows 600 a minute.
        kv_reads: std::cell::Cell<u32>,
        /// Reads allowed per wall-clock minute, as the host's broker does;
        /// `None` for no limit.
        read_quota: Option<u32>,
        /// The minute `window_reads` counts.
        read_window: std::cell::Cell<u64>,
        /// Reads in the current minute, by the sync or anything else.
        window_reads: std::cell::Cell<u32>,
        /// Reads the quota refused.
        refused_reads: std::cell::Cell<u32>,
        /// From the first write to this key on, every write is refused as
        /// over quota (a write quota spent part way through a run).
        refuse_writes_from: Option<String>,
        /// Once this entry is written, every later write (entry or
        /// key-value) is refused as over the quota.
        quota_after_put: Option<String>,
        /// Whether `refuse_writes_from` has been reached.
        refusing_writes: std::cell::Cell<bool>,
    }

    impl Fake {
        /// One read against the per-minute quota.
        fn read(&self) -> Result<(), String> {
            let minute = self.now / 60_000;
            if self.read_window.get() != minute {
                self.read_window.set(minute);
                self.window_reads.set(0);
            }
            if self
                .read_quota
                .is_some_and(|q| self.window_reads.get() >= q)
            {
                self.refused_reads.set(self.refused_reads.get() + 1);
                return Err(QUOTA_EXCEEDED.to_owned());
            }
            self.window_reads.set(self.window_reads.get() + 1);
            self.kv_reads.set(self.kv_reads.get() + 1);
            Ok(())
        }

        /// Reads spent this minute by other calls of the plugin (page renders).
        fn spend_reads(&self, n: u32) {
            let minute = self.now / 60_000;
            if self.read_window.get() != minute {
                self.read_window.set(minute);
                self.window_reads.set(0);
            }
            self.window_reads.set(self.window_reads.get() + n);
        }
    }

    fn blob_sha(content: &str) -> String {
        hex(&Sha256::digest(content.as_bytes()))[..40].to_owned()
    }

    impl Fake {
        fn tree_json(&self, commit: &str, tree_ref: &str, recursive: bool) -> Option<String> {
            let files = self.commits.get(commit)?;
            // tree_ref is the commit sha or "dir:<prefix>" synthesised below.
            let prefix = tree_ref.strip_prefix("dir:").unwrap_or("");
            let mut entries = Vec::new();
            let mut dirs = BTreeSet::new();
            for (path, content) in files {
                let Some(rest) = (if prefix.is_empty() {
                    Some(path.as_str())
                } else {
                    path.strip_prefix(&format!("{prefix}/"))
                }) else {
                    continue;
                };
                if recursive {
                    let mut acc = String::new();
                    let parts: Vec<&str> = rest.split('/').collect();
                    for seg in &parts[..parts.len() - 1] {
                        if !acc.is_empty() {
                            acc.push('/');
                        }
                        acc.push_str(seg);
                        if dirs.insert(acc.clone()) {
                            entries.push(serde_json::json!({"path": acc, "type": "tree", "sha": format!("dir:{}", if prefix.is_empty() { acc.clone() } else { format!("{prefix}/{acc}") })}));
                        }
                    }
                    entries.push(serde_json::json!({"path": rest, "type": "blob", "sha": blob_sha(content), "size": content.len()}));
                } else if let Some((d, _)) = rest.split_once('/') {
                    if dirs.insert(d.to_owned()) {
                        entries.push(serde_json::json!({"path": d, "type": "tree", "sha": format!("dir:{}", if prefix.is_empty() { d.to_owned() } else { format!("{prefix}/{d}") })}));
                    }
                } else {
                    entries.push(serde_json::json!({"path": rest, "type": "blob", "sha": blob_sha(content), "size": content.len()}));
                }
            }
            Some(
                serde_json::json!({"sha": tree_ref, "truncated": false, "tree": entries})
                    .to_string(),
            )
        }
    }

    struct Wrap<'a>(&'a RefCell<Fake>);

    impl Env for Wrap<'_> {
        fn get(&mut self, url: &str, headers: &[(&str, String)]) -> Result<Reply, String> {
            let mut f = self.0.borrow_mut();
            f.requests.push(url.to_owned());
            f.now += f.step_ms;
            f.etag_seen.push(
                headers
                    .iter()
                    .find(|(k, _)| *k == "if-none-match")
                    .map(|(_, v)| v.clone()),
            );
            if let Some(s) = f.fail_status {
                return Ok(Reply {
                    status: s,
                    etag: None,
                    body: String::new(),
                });
            }
            if url.contains("/commits/") {
                let inm = headers
                    .iter()
                    .find(|(k, _)| *k == "if-none-match")
                    .map(|(_, v)| v.clone());
                let r = url.rsplit('/').next().unwrap_or("");
                let head = f.refs.get(r).unwrap_or(&f.head).clone();
                let etag = format!("\"{head}\"");
                if inm.as_deref() == Some(etag.as_str()) {
                    return Ok(Reply {
                        status: 304,
                        etag: None,
                        body: String::new(),
                    });
                }
                return Ok(Reply {
                    status: 200,
                    etag: Some(etag),
                    body: head,
                });
            }
            if let Some(rest) = url.strip_prefix("https://api.github.com/repos/o/r/git/trees/") {
                let (r, recursive) = rest
                    .strip_suffix("?recursive=1")
                    .map_or((rest, false), |r| (r, true));
                let tree_ref = r.replace("%3A", ":");
                if f.commits.contains_key(&tree_ref) {
                    f.walking.clone_from(&tree_ref);
                }
                let commit = if f.walking.is_empty() {
                    f.head.clone()
                } else {
                    f.walking.clone()
                };
                let body = f
                    .tree_json(&commit, &tree_ref, recursive)
                    .ok_or("no such tree")?;
                return Ok(Reply {
                    status: 200,
                    etag: None,
                    body,
                });
            }
            if let Some(rest) = url.strip_prefix("https://raw.githubusercontent.com/o/r/") {
                let (commit, path) = rest.split_once('/').ok_or("bad raw url")?;
                let path = path.replace("%20", " ");
                return match f.commits.get(commit).and_then(|fs| fs.get(&path)) {
                    Some(c) => Ok(Reply {
                        status: 200,
                        etag: None,
                        body: c.clone(),
                    }),
                    None => Ok(Reply {
                        status: 404,
                        etag: None,
                        body: String::new(),
                    }),
                };
            }
            Err(format!("unexpected url {url}"))
        }
        fn now_ms(&self) -> u64 {
            self.0.borrow().now
        }
        fn kv_get(&mut self, key: &str) -> Result<Option<String>, String> {
            let f = self.0.borrow();
            f.read()?;
            let v = f.kv.borrow().get(key).cloned();
            Ok(v)
        }
        fn kv_set(&mut self, key: &str, value: &str) -> Result<(), String> {
            // As the host does (`KV_MAX_VALUE`).
            if value.len() > 64 * 1024 {
                return Err(format!("value exceeds {} bytes", 64 * 1024));
            }
            {
                let f = self.0.borrow();
                if f.refuse_writes_from.as_deref() == Some(key) {
                    f.refusing_writes.set(true);
                }
                if f.refusing_writes.get() {
                    return Err(QUOTA_EXCEEDED.to_owned());
                }
            }
            self.0
                .borrow()
                .kv
                .borrow_mut()
                .insert(key.to_owned(), value.to_owned());
            Ok(())
        }
        fn kv_delete(&mut self, key: &str) {
            self.0.borrow().kv.borrow_mut().remove(key);
        }
        fn kv_list(&mut self, prefix: &str) -> Result<Vec<String>, String> {
            let f = self.0.borrow();
            f.read()?;
            let keys: Vec<String> =
                f.kv.borrow()
                    .keys()
                    .filter(|k| k.starts_with(prefix))
                    .cloned()
                    .collect();
            Ok(keys)
        }
        fn put_entry(&mut self, e: &Entry) -> Result<Put, String> {
            let mut f = self.0.borrow_mut();
            if f.refusing_writes.get() && f.quota_after_put.is_some() {
                return Err(QUOTA_EXCEEDED.to_owned());
            }
            if f.refuse_keys.contains(&e.key) {
                return Err(format!(
                    "validation failed: blocks[$[0]]: {} refused",
                    e.key
                ));
            }
            for k in &f.refuse_kinds {
                if e.blocks_json.contains(&format!("\"kind\":\"{k}\"")) {
                    return Err(format!("validation failed: blocks[$[0]]: {k} refused"));
                }
            }
            if let Some(parent) = &e.parent_key {
                if !f.entries.contains_key(parent) {
                    return Err(format!("parent {parent} has not been written"));
                }
            }
            if f.quota_after_put.as_deref() == Some(e.key.as_str()) {
                f.refusing_writes.set(true);
            }
            match f.entries.get(&e.key) {
                Some((_, h)) if *h == e.source_hash => Ok(Put::Unchanged),
                Some(_) => {
                    f.entries
                        .insert(e.key.clone(), (e.clone(), e.source_hash.clone()));
                    Ok(Put::Updated)
                }
                None => {
                    f.entries
                        .insert(e.key.clone(), (e.clone(), e.source_hash.clone()));
                    Ok(Put::Created)
                }
            }
        }
        fn list_managed(&mut self) -> Result<Vec<Managed>, String> {
            Ok(self
                .0
                .borrow()
                .entries
                .iter()
                .map(|(k, (_, h))| Managed {
                    key: k.clone(),
                    source_hash: h.clone(),
                })
                .collect())
        }
        fn delete_entry(&mut self, key: &str) -> Result<(), String> {
            self.0.borrow_mut().entries.remove(key);
            Ok(())
        }
    }

    fn cfg() -> Config {
        Config {
            repo: github::parse_repo("https://github.com/o/r").unwrap(),
            branch: "main".into(),
            folder: vec!["docs".into()],
            token: None,
            latest_version: String::new(),
            versions: Vec::new(),
            version_refs: BTreeMap::new(),
            unreleased: Vec::new(),
            sections: Vec::new(),
        }
    }

    fn repo_one() -> Files {
        let mut f = Files::new();
        f.insert("README.md".into(), "# Repo readme\n".into());
        f.insert("docs/intro.md".into(), "---\ntitle: Introduction\nposition: 1\n---\n# ignored\n\nSee [install](guides/install.md) and [the host](../crates/host.wit).\n".into());
        f.insert(
            "docs/guides/install.md".into(),
            "# Install\n\n<Callout type=\"tip\">Fast.</Callout>\n".into(),
        );
        f.insert(
            "docs/guides/_meta.json".into(),
            r#"{"pages":["install"]}"#.into(),
        );
        f
    }

    fn fake_with(files: Files) -> RefCell<Fake> {
        let mut commits = BTreeMap::new();
        commits.insert(SHA1.to_owned(), files);
        RefCell::new(Fake {
            commits,
            head: SHA1.to_owned(),
            now: 1_700_000_000_000,
            step_ms: 1,
            ..Fake::default()
        })
    }

    /// Pages and `_meta.json` files in the packed scan cache.
    fn cached_scans(f: &RefCell<Fake>) -> usize {
        let fk = f.borrow();
        let kv = fk.kv.borrow();
        kv.iter()
            .filter(|(k, _)| k.starts_with("scanpack/"))
            .map(|(_, v)| {
                serde_json::from_str::<BTreeMap<String, String>>(v)
                    .unwrap()
                    .len()
            })
            .sum()
    }

    fn listed(r: &Report) -> Vec<String> {
        r.problems.clone().unwrap_or_default()
    }

    fn budget(f: &RefCell<Fake>) -> Budget {
        Budget {
            deadline_ms: f.borrow().now + 60_000,
            max_requests: 200,
            max_writes: 200,
        }
    }

    #[test]
    fn a_first_run_imports_folders_as_hierarchy_with_rewritten_links() {
        let f = fake_with(repo_one());
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(report.finished, "{report:?}");
        assert_eq!(report.created, 3, "{report:?}"); // intro, the guides group, install
        let fk = f.borrow();
        let intro = &fk.entries["main:intro.md"].0;
        assert_eq!(intro.title, "Introduction");
        assert!(
            intro
                .blocks_json
                .contains("href=\\\"/docs/guides/install\\\""),
            "{}",
            intro.blocks_json
        );
        assert!(intro.blocks_json.contains("https://github.com/o/r/blob/1111111111111111111111111111111111111111/crates/host.wit"), "{}", intro.blocks_json);
        assert!(intro.edit_url.ends_with("/edit/main/docs/intro.md"));
        let group = &fk.entries["@dir/main/guides"].0;
        assert_eq!(group.parent_key, None);
        let install = &fk.entries["main:guides/install.md"].0;
        assert_eq!(install.parent_key.as_deref(), Some("@dir/main/guides"));
        assert_eq!(install.title, "Install");
        assert!(install.blocks_json.contains("\"callout\""));
        assert_eq!(
            fk.kv.borrow().get("sync/complete").map(String::as_str),
            Some("1")
        );
        let raw = fk
            .requests
            .iter()
            .filter(|u| u.contains("raw.githubusercontent.com"))
            .count();
        assert_eq!(
            raw, 3,
            "each page and _meta.json is fetched once: {:?}",
            fk.requests
        );
    }

    #[test]
    fn a_second_run_with_no_new_commit_is_one_conditional_request() {
        let f = fake_with(repo_one());
        let b = budget(&f);
        run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        let before = f.borrow().requests.len();
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(report.up_to_date && report.finished, "{report:?}");
        assert_eq!(f.borrow().requests.len() - before, 1);
        assert_eq!(report.created + report.updated, 0);
    }

    #[test]
    fn a_sync_by_an_older_importer_runs_again_on_the_same_commit() {
        let f = fake_with(repo_one());
        let b = budget(&f);
        run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        // What a site synced by the previous plugin version holds: complete,
        // with no format recorded (or an older one).
        f.borrow().kv.borrow_mut().remove("sync/format");
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(!report.up_to_date && report.finished, "{report:?}");
        assert_eq!(
            f.borrow()
                .kv
                .borrow()
                .get("sync/format")
                .map(String::as_str),
            Some(FORMAT)
        );
    }

    #[test]
    fn a_page_the_host_refuses_is_imported_as_plain_text_and_listed() {
        let f = fake_with(repo_one());
        f.borrow_mut().refuse_kinds = vec!["callout"];
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(report.finished, "{report:?}");
        assert_eq!(
            listed(&report),
            ["docs/guides/install.md: imported as plain text \
              (validation failed: blocks[$[0]]: callout refused)"]
        );
        let blocks: serde_json::Value =
            serde_json::from_str(&f.borrow().entries["main:guides/install.md"].0.blocks_json)
                .unwrap();
        assert_eq!(
            blocks,
            serde_json::json!([{"kind": "code", "attrs": {"code": "# Install\n\n<Callout type=\"tip\">Fast.</Callout>", "language": null}, "children": []}])
        );
        assert!(f.borrow().entries.contains_key("main:intro.md"));
        // The list is kept while the page is unchanged ...
        let b = budget(&f);
        let again = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(again.up_to_date && again.problems.is_none(), "{again:?}");
        let kept = crate::status::Status::after(
            &crate::status::Status::after(&crate::status::Status::default(), 1, Ok(&report)),
            2,
            Ok(&again),
        );
        assert_eq!(kept.problems, listed(&report));
        // ... and goes once the page is written as it is.
        {
            let mut fk = f.borrow_mut();
            let mut files = fk.commits[SHA1].clone();
            files.insert(
                "docs/guides/install.md".into(),
                "# Install\n\nFixed.\n".into(),
            );
            fk.commits.insert(SHA2.into(), files);
            fk.head = SHA2.into();
        }
        let b = budget(&f);
        let fixed = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(fixed.finished && listed(&fixed).is_empty(), "{fixed:?}");
        assert!(!f.borrow().kv.borrow().contains_key(PROBLEMS_KEY));
    }

    #[test]
    fn a_page_refused_even_as_plain_text_is_left_out_and_the_sync_goes_on() {
        let f = fake_with(repo_one());
        f.borrow_mut().refuse_kinds = vec!["callout", "code"];
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(report.finished, "{report:?}");
        assert_eq!(
            listed(&report),
            ["docs/guides/install.md: not imported \
              (validation failed: blocks[$[0]]: callout refused)"]
        );
        assert!(!f.borrow().entries.contains_key("main:guides/install.md"));
        assert!(f.borrow().entries.contains_key("main:intro.md"));
    }

    #[test]
    fn the_children_of_a_refused_entry_are_left_out_and_listed() {
        // The generated Guides page: the parent of install.
        let probe = fake_with(repo_one());
        let b = budget(&probe);
        run(&mut Wrap(&probe), &cfg(), None, b).unwrap();
        let guides = probe.borrow().entries["main:guides/install.md"]
            .0
            .parent_key
            .clone()
            .unwrap();
        let f = fake_with(repo_one());
        f.borrow_mut().refuse_keys = vec![guides.clone()];
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(report.finished, "{report:?}");
        let problems = listed(&report);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems
            .iter()
            .any(|p| p.starts_with(&format!("{guides}: not imported (validation failed"))));
        assert!(problems.iter().any(|p| p.starts_with(&format!(
            "docs/guides/install.md: not imported (its parent {guides} is not on the site)"
        ))));
        assert!(f.borrow().entries.contains_key("main:intro.md"));
    }

    #[test]
    fn a_page_imported_as_plain_text_stays_listed_when_the_quota_runs_out_after_it() {
        let f = fake_with(repo_one());
        {
            let mut fk = f.borrow_mut();
            fk.refuse_kinds = vec!["callout"];
            fk.quota_after_put = Some("main:guides/install.md".into());
        }
        let b = budget(&f);
        let first = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(first.throttled, "{first:?}");
        assert!(f.borrow().entries.contains_key("main:guides/install.md"));
        {
            let mut fk = f.borrow_mut();
            fk.quota_after_put = None;
            fk.refusing_writes.set(false);
        }
        let b = budget(&f);
        let again = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(again.finished, "{again:?}");
        assert_eq!(
            listed(&again),
            ["docs/guides/install.md: imported as plain text \
              (validation failed: blocks[$[0]]: callout refused)"]
        );
    }

    #[test]
    fn plain_text_pages_left_off_a_full_list_are_listed_once_there_is_room() {
        let n = MAX_PROBLEMS + 10;
        let mut files = Files::new();
        for i in 0..n {
            files.insert(
                format!("docs/p{i:03}.md"),
                format!("# P{i}\n\n<Callout>x</Callout>\n"),
            );
        }
        let f = fake_with(files.clone());
        f.borrow_mut().refuse_kinds = vec!["callout"];
        let finish = |f: &RefCell<Fake>| {
            for _ in 0..20 {
                let b = budget(f);
                let r = run(&mut Wrap(f), &cfg(), None, b).unwrap();
                if r.finished {
                    return r;
                }
            }
            panic!("the sync never finishes");
        };
        let first = finish(&f);
        assert!(first.more_problems, "{first:?}");
        let shown = listed(&first);
        assert_eq!(shown.len(), MAX_PROBLEMS);
        // The listed pages are fixed; the others are still plain text.
        let mut fixed = files;
        for line in &shown {
            let path = line.split(':').next().unwrap();
            fixed.insert(path.to_owned(), "# Fixed\n".into());
        }
        {
            let mut fk = f.borrow_mut();
            fk.commits.insert(SHA2.into(), fixed);
            fk.head = SHA2.into();
        }
        let after = finish(&f);
        let rest = listed(&after);
        assert_eq!(rest.len(), 10, "{after:?}");
        assert!(!after.more_problems, "{after:?}");
        for line in &rest {
            assert!(!shown.contains(line), "{line}");
            assert!(line.contains("imported as plain text"), "{line}");
        }
    }

    #[test]
    fn pages_left_out_are_not_offered_again_so_the_sync_finishes() {
        let mut files = Files::new();
        for i in 0..30 {
            files.insert(
                format!("docs/p{i:02}.md"),
                format!("# P{i}\n\n<Callout>x</Callout>\n"),
            );
        }
        // A refused folder: its pages are listed without being read.
        files.insert("docs/big/_meta.json".into(), r#"{"title":"Big"}"#.into());
        for i in 0..30 {
            files.insert(format!("docs/big/q{i:02}.md"), format!("# Q{i}\n"));
        }
        let probe = fake_with(files.clone());
        let b = budget(&probe);
        run(&mut Wrap(&probe), &cfg(), None, b).unwrap();
        let big = probe.borrow().entries["main:big/q00.md"]
            .0
            .parent_key
            .clone()
            .unwrap();
        let f = fake_with(files);
        {
            let mut fk = f.borrow_mut();
            fk.refuse_kinds = vec!["callout", "code"];
            fk.refuse_keys = vec![big];
        }
        let mut runs = 0;
        let last = loop {
            runs += 1;
            assert!(runs <= 12, "the sync never finishes");
            let b = Budget {
                max_requests: 12,
                ..budget(&f)
            };
            let r = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
            if r.finished {
                break r;
            }
        };
        let problems = listed(&last);
        assert_eq!(problems.len(), 61, "{problems:?}");
        assert!(problems
            .iter()
            .any(|p| p.starts_with("docs/p00.md: not imported (validation failed")));
        // The next run of the same commit is up to date and keeps the list.
        let b = budget(&f);
        let r = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(r.up_to_date, "{r:?}");
    }

    #[test]
    fn the_problems_list_and_the_status_stay_under_the_store_limit() {
        let mut problems = Problems {
            map: BTreeMap::new(),
            full: false,
            dirty: false,
        };
        let long_name = "文".repeat(210);
        let reason = format!("validation failed: \"{}\"", "鍵\"".repeat(300));
        for i in 0..150 {
            let key = format!("{i:03}{}", "キ".repeat(200));
            problems.set(&key, &long_name, "not imported", &reason);
        }
        assert!(problems.full);
        assert!(problems.map.keys().all(|k| k.len() <= MAX_LISTED_KEY_BYTES));
        assert!(problems.serialized().len() <= MAX_PROBLEMS_BYTES);
        let r = Report {
            problems: Some(problems.lines()),
            more_problems: problems.full,
            ..Report::default()
        };
        let s = crate::status::Status::after(&crate::status::Status::default(), 1, Ok(&r));
        assert!(s.to_json().len() < 64 * 1024);
        assert!(s.line(1).contains(" or more: "));
        // A key the host refuses for its length is still found again.
        let key = format!("000{}", "キ".repeat(200));
        assert!(problems.contains(&key));
        problems.clear(&key);
        assert!(!problems.contains(&key));
    }

    #[test]
    fn a_storage_fault_still_stops_the_run() {
        assert!(!is_refusal("storage error during put-entry"));
        assert!(!is_refusal("quota-exceeded: writes"));
        assert!(is_refusal(
            "validation failed: blocks[$[4]]: invalid code language \"rust,ignore\""
        ));
        assert!(is_refusal("title must be 1..=200 characters"));
    }

    #[test]
    fn a_new_commit_rewrites_only_what_changed() {
        let f = fake_with(repo_one());
        let b = budget(&f);
        run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        {
            let mut fk = f.borrow_mut();
            let mut files = fk.commits[SHA1].clone();
            files.insert(
                "docs/guides/install.md".into(),
                "# Install\n\nChanged body.\n".into(),
            );
            fk.commits.insert(SHA2.into(), files);
            fk.head = SHA2.into();
        }
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(report.finished);
        // The page itself, and the generated Guides page that shows its first paragraph.
        assert_eq!(report.updated, 2, "{report:?}");
        assert_eq!(report.created, 0);
        assert!(report.unchanged >= 1);
        // intro links to install; its own text did not change, nor did any path.
        assert!(f.borrow().entries["main:guides/install.md"]
            .0
            .blocks_json
            .contains("Changed body"));
    }

    #[test]
    fn a_write_refused_while_recording_a_new_commit_leaves_it_unfinished() {
        for key in ["sync/complete", "sync/head/0", "sync/commit"] {
            let f = fake_with(repo_one());
            let b = budget(&f);
            assert!(run(&mut Wrap(&f), &cfg(), None, b).unwrap().finished);
            {
                let mut fk = f.borrow_mut();
                let mut files = fk.commits[SHA1].clone();
                files.insert(
                    "docs/guides/install.md".into(),
                    "# Install\n\nChanged body.\n".into(),
                );
                fk.commits.insert(SHA2.into(), files);
                fk.head = SHA2.into();
                fk.refuse_writes_from = Some(key.to_owned());
                fk.now += 60_000;
            }
            let b = budget(&f);
            let report = run(&mut Wrap(&f), &cfg(), None, b)
                .unwrap_or_else(|e| panic!("{key}: a refused write must not fail the run: {e}"));
            assert!(!report.finished, "{key}");
            {
                let fk = f.borrow();
                let kv = fk.kv.borrow();
                // Never "finished" beside a commit that was not synced.
                assert!(
                    kv.get("sync/complete").map(String::as_str) != Some("1")
                        || kv.get("sync/commit").map(String::as_str) == Some(SHA1),
                    "{key}: complete = 1 for commit {:?}",
                    kv.get("sync/commit")
                );
            }
            {
                let mut fk = f.borrow_mut();
                fk.refuse_writes_from = None;
                fk.refusing_writes.set(false);
                fk.now += 60_000;
            }
            let b = budget(&f);
            let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
            assert!(report.finished, "{key}: {report:?}");
            assert!(
                f.borrow().entries["main:guides/install.md"]
                    .0
                    .blocks_json
                    .contains("Changed body"),
                "{key}"
            );
        }
    }

    #[test]
    fn a_commit_that_changes_no_documentation_reads_no_pages() {
        let f = fake_with(repo_one());
        let b = budget(&f);
        run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        {
            // Only the repository README changes: same docs folder, new commit.
            let mut fk = f.borrow_mut();
            let mut files = fk.commits[SHA1].clone();
            files.insert("README.md".into(), "# Repo readme, edited\n".into());
            fk.commits.insert(SHA2.into(), files);
            fk.head = SHA2.into();
        }
        let before = f.borrow().requests.len();
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(report.finished, "{report:?}");
        assert_eq!(report.created + report.updated, 0, "{report:?}");
        let fk = f.borrow();
        let raw = fk.requests[before..]
            .iter()
            .filter(|u| u.contains("raw.githubusercontent.com"))
            .count();
        assert_eq!(raw, 0, "no page was fetched: {:?}", &fk.requests[before..]);
    }

    #[test]
    fn scans_of_files_that_are_gone_are_forgotten() {
        let f = fake_with(repo_one());
        let b = budget(&f);
        run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        let count = |f: &RefCell<Fake>| cached_scans(f);
        assert_eq!(
            count(&f),
            3,
            "intro.md, install.md and the _meta.json beside it"
        );
        {
            let mut fk = f.borrow_mut();
            let mut files = fk.commits[SHA1].clone();
            files.remove("docs/guides/install.md");
            fk.commits.insert(SHA2.into(), files);
            fk.head = SHA2.into();
        }
        let b = budget(&f);
        run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert_eq!(count(&f), 2, "intro.md and the _meta.json");
    }

    #[test]
    fn a_deleted_file_is_trashed_and_a_rename_moves_the_links() {
        let f = fake_with(repo_one());
        let b = budget(&f);
        run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        {
            let mut fk = f.borrow_mut();
            let mut files = fk.commits[SHA1].clone();
            files.remove("docs/guides/install.md");
            files.insert("docs/guides/setup.md".into(), "# Setup\n".into());
            files.insert(
                "docs/guides/_meta.json".into(),
                r#"{"pages":["setup"]}"#.into(),
            );
            files.insert(
                "docs/intro.md".into(),
                "---\ntitle: Introduction\nposition: 1\n---\nSee [setup](guides/setup.md).\n"
                    .into(),
            );
            fk.commits.insert(SHA2.into(), files);
            fk.head = SHA2.into();
        }
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert_eq!(report.deleted, 1, "{report:?}");
        let fk = f.borrow();
        assert!(!fk.entries.contains_key("main:guides/install.md"));
        assert!(fk.entries["main:intro.md"]
            .0
            .blocks_json
            .contains("/docs/guides/setup"));
    }

    #[test]
    fn running_out_of_budget_stops_cleanly_and_the_next_run_finishes() {
        let f = fake_with(repo_one());
        let tiny = Budget {
            deadline_ms: f.borrow().now + 60_000,
            max_requests: 200,
            max_writes: 1,
        };
        let first = run(&mut Wrap(&f), &cfg(), None, tiny).unwrap();
        assert!(!first.finished, "{first:?}");
        assert_eq!(f.borrow().entries.len(), 1);
        assert_eq!(
            f.borrow()
                .kv
                .borrow()
                .get("sync/complete")
                .map(String::as_str),
            Some("0")
        );
        let b = budget(&f);
        let second = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(second.finished, "{second:?}");
        assert_eq!(f.borrow().entries.len(), 3);
    }

    #[test]
    fn a_request_budget_that_runs_out_during_the_scan_resumes_from_the_cache() {
        let f = fake_with(repo_one());
        let tiny = Budget {
            deadline_ms: f.borrow().now + 60_000,
            max_requests: 5,
            max_writes: 200,
        };
        let first = run(&mut Wrap(&f), &cfg(), None, tiny).unwrap();
        assert!(!first.finished);
        let cached = cached_scans(&f);
        assert!(cached >= 1, "pages read so far are remembered");
        let b = budget(&f);
        let second = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(second.finished, "{second:?}");
        assert_eq!(f.borrow().entries.len(), 3);
    }

    #[test]
    fn a_round_cut_short_does_not_list_the_trees_again() {
        let f = fake_with(repo_one());
        let tiny = Budget {
            deadline_ms: f.borrow().now + 60_000,
            max_requests: 5,
            max_writes: 200,
        };
        assert!(!run(&mut Wrap(&f), &cfg(), None, tiny).unwrap().finished);
        let api = |f: &RefCell<Fake>| {
            f.borrow()
                .requests
                .iter()
                .filter(|u| u.contains("/git/trees/"))
                .count()
        };
        let listed = api(&f);
        assert!(listed >= 1);
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &cfg(), None, b).unwrap().finished);
        assert_eq!(api(&f), listed, "the second round listed again");
    }

    /// A site of sections only, with Folder empty: before, the main tree was
    /// the repository root and every section's pages were listed twice.
    #[test]
    fn a_sections_only_site_with_an_empty_folder_lists_each_tree_once() {
        let mut files = Files::new();
        files.insert("README.md".into(), "# Repo readme\n".into());
        for i in 0..5 {
            files.insert(format!("docs/page{i}.md"), format!("# Page {i}\n"));
        }
        let sections = vec![crate::settings::SectionDef {
            slug: "engine".into(),
            title: "Engine".into(),
            folder: vec!["docs".into()],
            git_ref: None,
        }];
        let pages = |f: &RefCell<Fake>| -> (usize, usize) {
            let fk = f.borrow();
            let md = fk.entries.keys().filter(|k| k.ends_with(".md"));
            let (sec, other): (Vec<_>, Vec<_>) = md.partition(|k| k.starts_with("s:"));
            (sec.len(), other.len())
        };
        let big = |f: &RefCell<Fake>| Budget {
            deadline_ms: f.borrow().now + 60_000,
            max_requests: 5000,
            max_writes: 5000,
        };

        let f = fake_with(files.clone());
        let mut c = cfg();
        c.folder = Vec::new();
        c.sections = sections.clone();
        let b = big(&f);
        assert!(run(&mut Wrap(&f), &c, None, b).unwrap().finished);
        assert_eq!(pages(&f), (5, 0), "only the section's pages, once");

        // With an older version configured the main tree stays: its versions belong to it.
        let f = fake_with(files.clone());
        c.versions = vec![("v1".into(), vec!["docs".into()])];
        let b = big(&f);
        assert!(run(&mut Wrap(&f), &c, None, b).unwrap().finished);
        assert!(
            pages(&f).1 > 0,
            "the main tree is listed beside its versions"
        );

        // Without sections an empty Folder is still the repository root.
        let f = fake_with(files);
        let mut c = cfg();
        c.folder = Vec::new();
        let b = big(&f);
        assert!(run(&mut Wrap(&f), &c, None, b).unwrap().finished);
        assert_eq!(pages(&f), (0, 6), "the repository root is the main tree");
    }

    /// The broker allows 600 key-value reads a minute, extended task or not.
    /// A site with two sections of about 620 pages each (one folder each)
    /// must sync inside that, on the first run, on a run that
    /// changes a page, and on a run that changes none.
    #[test]
    fn a_site_of_over_a_thousand_pages_syncs_within_the_key_value_read_quota() {
        let mut files = Files::new();
        for i in 0..620 {
            files.insert(
                format!("docs/area{}/page{i}.md", i % 25),
                format!("---\ntitle: Page {i}\n---\n# Page {i}\n\nBody {i}.\n"),
            );
        }
        let f = fake_with(files);
        let mut c = cfg();
        c.sections = vec![
            crate::settings::SectionDef {
                slug: "engine/0.2".into(),
                title: "Engine 0.2".into(),
                folder: vec!["docs".into()],
                git_ref: None,
            },
            crate::settings::SectionDef {
                slug: "engine/next".into(),
                title: "Engine next".into(),
                folder: vec!["docs".into()],
                git_ref: None,
            },
        ];
        let reads = |f: &RefCell<Fake>| f.borrow().kv_reads.replace(0);
        let big = |f: &RefCell<Fake>| Budget {
            deadline_ms: f.borrow().now + 60_000,
            max_requests: 5000,
            max_writes: 5000,
        };
        let b = big(&f);
        let first = run(&mut Wrap(&f), &c, None, b).unwrap();
        assert!(first.finished, "{first:?}");
        assert!(reads(&f) < 100, "first run read too much");
        // A page changes: a new commit.
        {
            let mut fk = f.borrow_mut();
            let mut files = fk.commits[SHA1].clone();
            files.insert("docs/area0/page0.md".into(), "# Changed\n".into());
            fk.commits.insert(SHA2.into(), files);
            fk.head = SHA2.into();
        }
        let b = big(&f);
        let second = run(&mut Wrap(&f), &c, None, b).unwrap();
        assert!(second.finished, "{second:?}");
        assert!(reads(&f) < 100, "a run after a commit read too much");
        // Cut short by the request budget, then resumed: still few reads.
        f.borrow()
            .kv
            .borrow_mut()
            .retain(|k, _| !k.starts_with("scanpack/") && !k.starts_with("sync/"));
        let tiny = Budget {
            deadline_ms: f.borrow().now + 60_000,
            max_requests: 100,
            max_writes: 5000,
        };
        let cut = run(&mut Wrap(&f), &c, None, tiny).unwrap();
        assert!(!cut.finished);
        assert!(reads(&f) < 100);
        let mut done = false;
        for _ in 0..20 {
            let b = big(&f);
            let r = run(&mut Wrap(&f), &c, None, b).unwrap();
            assert!(reads(&f) < 100, "a resumed run read too much");
            if r.finished {
                done = true;
                break;
            }
        }
        assert!(done);
    }

    /// A large docs site: three versions of about 200
    /// pages each, two read from release tags, one from the branch.
    fn three_versions() -> (RefCell<Fake>, Config) {
        three_versions_of(200)
    }

    /// Three sections (two tags and the main branch) of `pages` pages each,
    /// beside the main tree of the same `pages`.
    fn three_versions_of(pages: usize) -> (RefCell<Fake>, Config) {
        const SHA_02: &str = "0202020202020202020202020202020202020202";
        const SHA_03: &str = "0303030303030303030303030303030303030303";
        let version = |v: &str| -> Files {
            let mut files = Files::new();
            for i in 0..pages {
                files.insert(
                    format!("docs/area{}/page{i}.md", i % 12),
                    format!("---\ntitle: Page {i}\n---\n# Page {i} ({v})\n\nBody {i} of {v}.\n"),
                );
            }
            files
        };
        let f = fake_with(version("next"));
        {
            let mut fk = f.borrow_mut();
            fk.commits.insert(SHA_02.into(), version("0.2"));
            fk.commits.insert(SHA_03.into(), version("0.3"));
            fk.refs.insert("v0.2.0".into(), SHA_02.into());
            fk.refs.insert("v0.3.0".into(), SHA_03.into());
            // The host's default: 600 reads a minute (an ordinary call's
            // budget; an extended task has more).
            fk.read_quota = Some(600);
        }
        let mut c = cfg();
        c.sections = [
            ("engine/0.2", Some("v0.2.0")),
            ("engine/0.3", Some("v0.3.0")),
            ("engine/next", None),
        ]
        .into_iter()
        .map(|(slug, git_ref)| crate::settings::SectionDef {
            slug: slug.into(),
            title: slug.into(),
            folder: vec!["docs".into()],
            git_ref: git_ref.map(Into::into),
        })
        .collect();
        (f, c)
    }

    /// The budget the plugin gives one scheduled extended task.
    fn task_budget(f: &RefCell<Fake>) -> Budget {
        Budget {
            deadline_ms: f.borrow().now + 55_000,
            max_requests: 250,
            max_writes: 500,
        }
    }

    fn raw_fetches(f: &RefCell<Fake>) -> usize {
        f.borrow()
            .requests
            .iter()
            .filter(|u| u.contains("raw.githubusercontent.com"))
            .count()
    }

    #[test]
    fn three_versions_of_two_hundred_pages_sync_under_the_default_read_quota() {
        let (f, c) = three_versions();
        let mut runs = 0;
        loop {
            runs += 1;
            assert!(runs <= 12, "the sync did not finish in 12 scheduled runs");
            let b = task_budget(&f);
            let r = run(&mut Wrap(&f), &c, None, b).unwrap();
            assert!(!r.throttled, "run {runs} was throttled: {r:?}");
            // The scheduler runs the task once a minute.
            f.borrow_mut().now += 60_000;
            if r.finished {
                break;
            }
        }
        let fk = f.borrow();
        assert_eq!(fk.refused_reads.get(), 0, "no read was refused");
        let pages = fk
            .entries
            .keys()
            .filter(|k| k.starts_with("s:") && k.ends_with(".md"))
            .count();
        assert_eq!(pages, 600, "every page of every version is imported");
    }

    /// A main tree and three sections of about 700 pages each (2,800
    /// documents) was refused before any write at 2,000.
    #[test]
    fn four_trees_of_seven_hundred_pages_are_listed_and_imported() {
        let (f, c) = three_versions_of(700);
        let mut runs = 0;
        let mut created = 0;
        loop {
            runs += 1;
            assert!(runs <= 40, "the sync did not finish in 40 scheduled runs");
            let b = task_budget(&f);
            let r = run(&mut Wrap(&f), &c, None, b).expect("2,800 documents are under the cap");
            assert!(!r.throttled, "run {runs} was throttled: {r:?}");
            created += r.created;
            f.borrow_mut().now += 60_000;
            if r.finished {
                break;
            }
        }
        let fk = f.borrow();
        let pages = fk
            .entries
            .keys()
            .filter(|k| k.starts_with("s:") && k.ends_with(".md"))
            .count();
        assert_eq!(pages, 2100, "every page of every version is imported");
        assert!(created > 0, "the sync writes pages");
    }

    #[test]
    fn more_documents_than_the_cap_are_refused_with_the_count() {
        let (f, c) = three_versions_of(MAX_PAGES / 4 + 1);
        let b = task_budget(&f);
        let e = run(&mut Wrap(&f), &c, None, b).unwrap_err();
        assert!(
            e.contains("documents found") && e.contains(&MAX_PAGES.to_string()),
            "{e}"
        );
    }

    /// The listing of a folder of a thousand pages is more than the
    /// key-value store keeps in one value (64 KiB). Kept whole, it was
    /// refused, so every run listed the folder again: two GitHub calls a
    /// tree a run, which ran an anonymous site out of its 60 calls an hour.
    /// Now only the documents are kept, in parts, and a later run of the
    /// same commit lists nothing.
    #[test]
    fn a_thousand_page_listing_is_kept_and_not_listed_again() {
        let mut files = Files::new();
        for i in 0..1000 {
            files.insert(
                format!("docs/reference/area{}/a-longer-page-name-{i:04}.md", i % 12),
                format!("---\ntitle: Page {i}\n---\n# Page {i}\n\nBody {i}.\n"),
            );
        }
        for a in 0..12 {
            files.insert(
                format!("docs/reference/area{a}/images/diagram.png"),
                "png".into(),
            );
        }
        let f = fake_with(files);
        f.borrow_mut().read_quota = Some(600);
        let c = cfg();
        let tree_calls = |f: &RefCell<Fake>| {
            f.borrow()
                .requests
                .iter()
                .filter(|u| u.contains("/git/trees/"))
                .count()
        };
        let mut runs = 0;
        let mut first = 0;
        loop {
            runs += 1;
            assert!(runs <= 20, "the sync did not finish in 20 scheduled runs");
            let b = task_budget(&f);
            let r = run(&mut Wrap(&f), &c, None, b).unwrap();
            assert!(!r.throttled, "run {runs} was throttled: {r:?}");
            if runs == 1 {
                first = tree_calls(&f);
            }
            f.borrow_mut().now += 60_000;
            if r.finished {
                break;
            }
        }
        assert!(runs > 1, "the import takes several runs");
        assert!(first > 0, "the first run lists the folder");
        assert_eq!(
            tree_calls(&f),
            first,
            "later runs of the same commit read the kept listing"
        );
        let fk = f.borrow();
        let kv = fk.kv.borrow();
        let parts: Vec<&String> = kv.keys().filter(|k| k.starts_with(TREE_PREFIX)).collect();
        assert!(parts.len() > 1, "the listing is kept in parts: {parts:?}");
        let pages = fk
            .entries
            .keys()
            .filter(|k| k.starts_with("main:") && k.ends_with(".md"))
            .count();
        assert_eq!(pages, 1000, "every page is imported");
    }

    /// Without a token GitHub allows 60 API calls an hour and a 304 is free.
    /// An unfinished sync asked for every head in full each minute, used the
    /// hour's calls within half an hour and then failed every run with 403.
    /// Now each head is fetched in full once; later runs ask with its etag.
    #[test]
    fn an_unfinished_sync_asks_for_its_heads_with_their_etags() {
        let (f, c) = three_versions();
        let mut runs = 0;
        loop {
            runs += 1;
            assert!(runs <= 12, "the sync did not finish in 12 scheduled runs");
            let b = task_budget(&f);
            let r = run(&mut Wrap(&f), &c, None, b).unwrap();
            f.borrow_mut().now += 60_000;
            if r.finished {
                assert!(
                    !r.up_to_date,
                    "an unchanged head does not end an unfinished sync"
                );
                break;
            }
        }
        assert!(runs > 1, "the import takes several runs");
        let fk = f.borrow();
        let commit_asks: Vec<&Option<String>> = fk
            .requests
            .iter()
            .zip(&fk.etag_seen)
            .filter(|(u, _)| u.contains("/commits/"))
            .map(|(_, e)| e)
            .collect();
        assert_eq!(commit_asks.len(), 3 * runs, "three refs a run");
        assert_eq!(
            commit_asks.iter().filter(|e| e.is_none()).count(),
            3,
            "each head is fetched in full once"
        );
        let pages = fk
            .entries
            .keys()
            .filter(|k| k.starts_with("s:") && k.ends_with(".md"))
            .count();
        assert_eq!(pages, 600);
    }

    /// Page renders spend most of the plugin's read budget. A run that the
    /// host refuses a read pauses (no error, nothing forgotten) and a later
    /// run carries on: the scan cache never shrinks and no run starts the
    /// import over.
    #[test]
    fn a_throttled_sync_pauses_and_resumes_where_it_stopped() {
        let (f, c) = three_versions();
        // The unthrottled cost, for comparison.
        let (g, _) = three_versions();
        loop {
            let b = task_budget(&g);
            let r = run(&mut Wrap(&g), &c, None, b).unwrap();
            g.borrow_mut().now += 60_000;
            if r.finished {
                break;
            }
        }
        let baseline = raw_fetches(&g);

        let mut throttled = 0;
        let mut finished = false;
        let mut cached = 0;
        // Each minute other calls leave the sync only `left` reads: from none
        // (refused at once) to enough for a whole run.
        for (minute, left) in [
            0u32, 1, 2, 3, 5, 8, 600, 4, 600, 6, 600, 7, 600, 600, 600, 600,
        ]
        .into_iter()
        .enumerate()
        {
            f.borrow().spend_reads(600 - left);
            let b = task_budget(&f);
            let r = run(&mut Wrap(&f), &c, None, b)
                .unwrap_or_else(|e| panic!("minute {minute}: a throttled run must not fail: {e}"));
            f.borrow_mut().now += 60_000;
            if r.throttled {
                throttled += 1;
                assert!(!r.finished);
            }
            let now_cached = cached_scans(&f);
            assert!(
                now_cached >= cached,
                "minute {minute}: the scan cache shrank ({cached} -> {now_cached})"
            );
            cached = now_cached;
            if r.finished {
                finished = true;
                break;
            }
        }
        assert!(throttled >= 5, "the quota refused {throttled} runs");
        assert!(finished, "the sync never finished");
        let pages = f
            .borrow()
            .entries
            .keys()
            .filter(|k| k.starts_with("s:") && k.ends_with(".md"))
            .count();
        assert_eq!(pages, 600, "every page of every version is imported");
        // Refused runs read no page: the pages fetched are the unthrottled
        // import's, give or take the bodies a run boundary drops.
        let fetched = raw_fetches(&f);
        assert!(
            fetched <= baseline + 250,
            "{fetched} page fetches against {baseline} unthrottled"
        );
    }

    #[test]
    fn errors_name_the_problem() {
        let f = fake_with(repo_one());
        f.borrow_mut().fail_status = Some(404);
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &cfg(), None, b)
            .unwrap_err()
            .contains("not found"));
        f.borrow_mut().fail_status = Some(403);
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &cfg(), None, b)
            .unwrap_err()
            .contains("rate limit"));
        f.borrow_mut().fail_status = Some(401);
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &cfg(), None, b)
            .unwrap_err()
            .contains("token"));
    }

    #[test]
    fn hidden_pages_and_folders_are_kept_out_of_search() {
        let mut files = repo_one();
        files.insert(
            "docs/secret.md".into(),
            "---\nhide: true\n---\n# Secret\n".into(),
        );
        files.insert(
            "docs/_meta.json".into(),
            r#"{"guides": {"title": "Guides", "hidden": true}}"#.into(),
        );
        let f = fake_with(files);
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &cfg(), None, b).unwrap().finished);
        let fk = f.borrow();
        let noindex = |key: &str| fk.entries[key].0.seo_json.contains("\"noindex\":true");
        assert!(noindex("main:secret.md"));
        assert!(noindex("main:guides/install.md"), "inside a hidden folder");
        assert!(!noindex("main:intro.md"));
    }

    #[test]
    fn a_missing_folder_is_reported() {
        let f = fake_with(repo_one());
        let mut c = cfg();
        c.folder = vec!["nope".into()];
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &c, None, b)
            .unwrap_err()
            .contains("folder nope"));
    }

    #[test]
    fn a_section_has_its_own_root_term_and_url_prefix() {
        let mut files = repo_one();
        files.insert("api/index.md".into(), "# API\n".into());
        files.insert("api/calls.md".into(), "# Calls\n".into());
        let f = fake_with(files);
        let mut c = cfg();
        c.sections = vec![crate::settings::SectionDef {
            slug: "api".into(),
            title: "API reference".into(),
            folder: vec!["api".into()],
            git_ref: None,
        }];
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &c, None, b).unwrap().finished);
        let fk = f.borrow();
        let root = &fk.entries["@section/api"].0;
        assert!(root.fields_json.contains("\"section_root\":true"));
        assert!(!root.fields_json.contains("\"version_root\":true"));
        assert_eq!(root.title, "API reference");
        let calls = &fk.entries["s:api:calls.md"].0;
        assert_eq!(calls.terms, ["doc-section:api"]);
        assert_eq!(calls.parent_key.as_deref(), Some("@section/api"));
        assert!(!calls.seo_json.contains("canonical"));
    }

    #[test]
    fn a_product_version_section_hangs_under_a_generated_product_root() {
        let mut files = repo_one();
        files.insert("api/index.md".into(), "# API\n".into());
        files.insert("api/calls.md".into(), "# Calls\n".into());
        let f = fake_with(files);
        let mut c = cfg();
        c.sections = vec![crate::settings::SectionDef {
            slug: "engine/0.2".into(),
            title: "Engine 0.2".into(),
            folder: vec!["api".into()],
            git_ref: None,
        }];
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &c, None, b).unwrap().finished);
        let fk = f.borrow();
        let product = &fk.entries["@section/engine"].0;
        assert_eq!(product.slug, "engine");
        assert!(product.parent_key.is_none());
        let root = &fk.entries["@section/engine/0.2"].0;
        assert_eq!(root.slug, "0-2");
        assert_eq!(root.parent_key.as_deref(), Some("@section/engine"));
        let calls = &fk.entries["s:engine/0.2:calls.md"].0;
        assert_eq!(calls.parent_key.as_deref(), Some("@section/engine/0.2"));
        assert_eq!(calls.terms, ["doc-section:engine-0-2"]);
    }

    fn engine_section(files: Files) -> (RefCell<Fake>, Config) {
        let f = fake_with(files);
        let mut c = cfg();
        c.sections = vec![crate::settings::SectionDef {
            slug: "engine/0.2".into(),
            title: "Engine 0.2".into(),
            folder: vec!["api".into()],
            git_ref: None,
        }];
        (f, c)
    }

    #[test]
    fn a_root_without_an_index_page_lists_its_children_as_cards() {
        let mut files = repo_one();
        files.insert(
            "api/start/quick.md".into(),
            "---\ndescription: Run a first query in two minutes.\n---\n# Quickstart\n\nIgnored body.\n".into(),
        );
        files.insert(
            "api/start/second.md".into(),
            "# Second steps\n\nLoad a *Parquet* file, then <b>query</b> it.\n\nMore.\n".into(),
        );
        let (f, c) = engine_section(files);
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &c, None, b).unwrap().finished);
        let fk = f.borrow();
        let group = fk
            .entries
            .iter()
            .find(|(k, _)| k.starts_with("@dir/") && k.ends_with("/start"))
            .map(|(_, v)| &v.0)
            .expect("the folder gets a generated page");
        assert!(group.fields_json.contains("\"cards\":true"));
        let text = &group.blocks_json;
        assert!(!text.contains("In this section"), "{text}");
        assert!(text.contains("Quickstart"), "{text}");
        assert!(text.contains("Run a first query in two minutes."), "{text}");
        assert!(!text.contains("Ignored body"), "description wins: {text}");
        assert!(text.contains("Second steps"), "{text}");
        assert!(
            text.contains("Load a Parquet file, then query it."),
            "no description: the first paragraph, as plain text: {text}"
        );
        assert!(!text.contains("More."), "{text}");
        // The root has no index page either: its children are the cards.
        let root = &fk.entries["@section/engine/0.2"].0;
        assert!(root.fields_json.contains("\"cards\":true"));
        assert!(root.blocks_json.contains("Start"), "{}", root.blocks_json);
        assert!(!root.blocks_json.contains("In this section"));
    }

    #[test]
    fn a_root_with_an_index_page_shows_it_and_is_not_a_card_list() {
        let mut files = repo_one();
        files.insert(
            "api/index.md".into(),
            "# Engine docs\n\nWelcome to the overview text.\n".into(),
        );
        files.insert("api/calls.md".into(), "# Calls\n".into());
        let (f, c) = engine_section(files);
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &c, None, b).unwrap().finished);
        let fk = f.borrow();
        let root = &fk.entries["@section/engine/0.2"].0;
        assert!(
            root.blocks_json.contains("Welcome to the overview text."),
            "{}",
            root.blocks_json
        );
        assert!(!root.fields_json.contains("cards"), "{}", root.fields_json);
        assert!(!root.blocks_json.contains("Documentation for"));
    }

    #[test]
    fn pages_from_before_the_sections_were_set_go_before_the_new_ones_are_written() {
        // First import: no sections yet, so the pages are unversioned.
        let f = fake_with(repo_one());
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &cfg(), None, b).unwrap().finished);
        assert!(f
            .borrow()
            .entries
            .keys()
            .any(|k| k.starts_with("s:") || k.starts_with("p:") || !k.starts_with("@section")));
        let old: Vec<String> = f.borrow().entries.keys().cloned().collect();
        // Settings now name a section over another folder; a run that can only
        // write one entry still removes every stale page.
        let mut files = repo_one();
        files.insert("api/calls.md".into(), "# Calls\n".into());
        files.insert("api/more.md".into(), "# More\n".into());
        files.insert("other/x.md".into(), "# X\n".into());
        {
            let mut fk = f.borrow_mut();
            fk.commits.insert(SHA2.into(), files);
            fk.head = SHA2.into();
        }
        let (_, mut c) = engine_section(repo_one());
        // The main folder is now another one: its old pages belong to nobody.
        c.folder = vec!["other".into()];
        let tight = |f: &RefCell<Fake>| Budget {
            deadline_ms: f.borrow().now + 60_000,
            max_requests: 200,
            max_writes: 2,
        };
        let b = tight(&f);
        let first = run(&mut Wrap(&f), &c, None, b).unwrap();
        assert_eq!(
            first.created, 0,
            "stale pages go before anything new is written: {first:?}"
        );
        assert_eq!(first.deleted, 2, "{first:?}");
        // Later runs resume where the last stopped and finish.
        for _ in 0..6 {
            let b = tight(&f);
            if run(&mut Wrap(&f), &c, None, b).unwrap().finished {
                break;
            }
        }
        let keys: Vec<String> = f.borrow().entries.keys().cloned().collect();
        for k in &old {
            assert!(!keys.contains(k), "stale {k} still there: {keys:?}");
        }
        assert!(keys.iter().any(|k| k.ends_with("x.md")), "{keys:?}");
    }

    /// The sidebar a reader gets from what the importer wrote: every page is a link.
    fn sidebar_links(f: &RefCell<Fake>, current_key: &str) -> (usize, usize) {
        let fk = f.borrow();
        let ids: BTreeMap<&str, usize> = fk
            .entries
            .keys()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i + 1))
            .collect();
        let tree: Vec<serde_json::Value> = fk
            .entries
            .iter()
            .map(|(k, (e, _))| {
                serde_json::json!({
                    "id": ids[k.as_str()].to_string(),
                    "parent_id": e.parent_key.as_deref().and_then(|p| ids.get(p)).map(ToString::to_string),
                    "position": e.position, "title": e.title, "slug": e.slug,
                    "url": format!("/docs/{}", e.slug),
                    "fields": serde_json::from_str::<serde_json::Value>(&e.fields_json).unwrap(),
                })
            })
            .collect();
        let mut ancestors = Vec::new();
        let mut cur = fk.entries[current_key].0.parent_key.clone();
        while let Some(p) = cur {
            ancestors.push(ids[p.as_str()].to_string());
            cur = fk.entries[&p].0.parent_key.clone();
        }
        ancestors.reverse();
        let payload = serde_json::json!({"context": {
            "entry": {"id": ids[current_key].to_string(), "ancestors": ancestors},
            "tree": tree, "toc": []}});
        let page = crate::nav::parse(&payload).expect("a page");
        let html = crate::nav::sidebar(&page, "Latest", &[]);
        let visible = fk
            .entries
            .values()
            .filter(|(e, _)| {
                // Only what hangs under the section the page is in.
                let mut cur = e.parent_key.as_deref();
                let mut below = false;
                while let Some(p) = cur {
                    if p == "@section/engine/0.2" {
                        below = true;
                        break;
                    }
                    cur = fk.entries.get(p).and_then(|x| x.0.parent_key.as_deref());
                }
                below
            })
            .filter(|(e, _)| {
                let f: serde_json::Value = serde_json::from_str(&e.fields_json).unwrap();
                ["hide", "section_root", "version_root"]
                    .iter()
                    .all(|k| f[*k].as_bool() != Some(true))
            })
            .count();
        (html.matches("docs-nav__link").count(), visible)
    }

    #[test]
    fn the_sidebar_lists_every_page_of_a_section() {
        for with_index in [false, true] {
            let mut files = repo_one();
            if with_index {
                files.insert("api/index.md".into(), "# Engine docs\n\nWelcome.\n".into());
            }
            files.insert("api/calls.md".into(), "# Calls\n".into());
            files.insert("api/start/quick.md".into(), "# Quickstart\n\nGo.\n".into());
            files.insert("api/start/second.md".into(), "# Second\n\nGo.\n".into());
            files.insert("api/start/deep/third.md".into(), "# Third\n".into());
            let (f, c) = engine_section(files);
            let b = budget(&f);
            assert!(run(&mut Wrap(&f), &c, None, b).unwrap().finished);
            let key = "s:engine/0.2:calls.md";
            let (links, visible) = sidebar_links(&f, key);
            assert!(visible >= 5, "{visible}");
            assert_eq!(
                links, visible,
                "index={with_index}: every page is a sidebar link"
            );
        }
    }

    #[test]
    fn a_folder_with_an_index_page_renders_that_page() {
        let mut files = repo_one();
        files.insert("api/index.md".into(), "# Top\n".into());
        files.insert(
            "api/guides/index.md".into(),
            "# Guides\n\nThe guides intro text.\n".into(),
        );
        files.insert("api/guides/one.md".into(), "# One\n".into());
        let (f, c) = engine_section(files);
        let b = budget(&f);
        assert!(run(&mut Wrap(&f), &c, None, b).unwrap().finished);
        let fk = f.borrow();
        let guides = &fk.entries["s:engine/0.2:guides/index.md"].0;
        assert!(guides.blocks_json.contains("The guides intro text."));
        assert!(!guides.fields_json.contains("cards"));
    }

    #[test]
    fn a_version_read_from_another_ref_uses_that_refs_commit_and_edit_link() {
        let mut next_files = repo_one();
        next_files.insert("docs/intro.md".into(), "# Intro, unreleased\n".into());
        next_files.insert("docs/new.md".into(), "# New\n".into());
        let f = fake_with(repo_one());
        f.borrow_mut().commits.insert(SHA2.to_owned(), next_files);
        f.borrow_mut()
            .refs
            .insert("dev".to_owned(), SHA2.to_owned());
        let mut c = cfg();
        c.latest_version = "v2".into();
        c.versions = vec![("next".into(), vec!["docs".into()])];
        c.version_refs.insert("next".into(), "dev".into());
        c.unreleased = vec!["next".into()];
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &c, None, b).unwrap();
        assert!(report.finished, "{report:?}");
        {
            let fk = f.borrow();
            assert!(
                fk.entries["v:next:new.md"]
                    .0
                    .seo_json
                    .contains("\"noindex\":true"),
                "an unreleased version stays out of search engines"
            );
            assert!(
                fk.entries.contains_key("v:next:new.md"),
                "{:?}",
                fk.entries.keys()
            );
            assert!(!fk.entries.contains_key("main:new.md"));
            assert!(fk.entries["v:next:new.md"]
                .0
                .edit_url
                .contains("/edit/dev/docs/new.md"));
            assert!(fk.entries["main:intro.md"]
                .0
                .edit_url
                .contains("/edit/main/"));
        }
        // Nothing moved: one conditional request per ref, nothing written.
        let before = f.borrow().requests.len();
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &c, None, b).unwrap();
        assert!(report.up_to_date, "{report:?}");
        assert_eq!(f.borrow().requests.len() - before, 2);
        // Only the other ref moves: the sync runs again.
        const SHA3: &str = "3333333333333333333333333333333333333333";
        let mut files = f.borrow().commits[SHA2].clone();
        files.insert("docs/newer.md".into(), "# Newer\n".into());
        f.borrow_mut().commits.insert(SHA3.to_owned(), files);
        f.borrow_mut()
            .refs
            .insert("dev".to_owned(), SHA3.to_owned());
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &c, None, b).unwrap();
        assert!(!report.up_to_date && report.finished, "{report:?}");
        assert!(f.borrow().entries.contains_key("v:next:newer.md"));
    }

    #[test]
    fn versions_nest_old_docs_under_a_version_entry_with_terms_and_canonicals() {
        let mut files = repo_one();
        files.insert("docs-v1/guides/install.md".into(), "# Install v1\n".into());
        files.insert("docs-v1/old.md".into(), "# Old\n".into());
        let f = fake_with(files);
        let mut c = cfg();
        c.latest_version = "v2".into();
        c.versions = vec![("v1".into(), vec!["docs-v1".into()])];
        let b = budget(&f);
        let report = run(&mut Wrap(&f), &c, None, b).unwrap();
        assert!(report.finished, "{report:?}");
        let fk = f.borrow();
        let root = &fk.entries["@version/v1"].0;
        assert!(root.fields_json.contains("\"version_root\":true"));
        assert_eq!(root.terms, ["doc-version:v1"]);
        let old_install = &fk.entries["v:v1:guides/install.md"].0;
        assert_eq!(old_install.terms, ["doc-version:v1"]);
        assert!(
            old_install.seo_json.contains("/docs/guides/install"),
            "{}",
            old_install.seo_json
        );
        assert_eq!(old_install.parent_key.as_deref(), Some("@dir/v1/guides"));
        let old = &fk.entries["v:v1:old.md"].0;
        assert_eq!(old.parent_key.as_deref(), Some("@version/v1"));
        assert_eq!(old.seo_json, "{}");
        let latest = &fk.entries["main:intro.md"].0;
        assert_eq!(latest.terms, ["doc-version:v2"]);
    }

    #[test]
    fn empty_field_values_are_left_out_not_sent_empty() {
        let f: serde_json::Value =
            serde_json::from_str(&doc_fields(true, false, "", "", "")).unwrap();
        assert_eq!(
            f,
            serde_json::json!({"hide_from_nav": false, "version_root": true})
        );
        let f: serde_json::Value = serde_json::from_str(&doc_fields(
            false,
            true,
            "Start",
            "https://x.test/e",
            "2026-10-15",
        ))
        .unwrap();
        assert_eq!(f["section_label"], "Start");
        assert_eq!(f["edit_url"], "https://x.test/e");
        assert_eq!(f["last_updated"], "2026-10-15");
    }

    #[test]
    fn dates_are_civil_utc() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(1_700_000_000_000), "2023-11-14");
        assert_eq!(civil_date(951_782_400_000), "2000-02-29");
    }

    #[test]
    fn tokens_go_only_to_the_right_header_per_host() {
        struct Spy(Vec<(String, Vec<(String, String)>)>);
        impl Env for Spy {
            fn get(&mut self, url: &str, headers: &[(&str, String)]) -> Result<Reply, String> {
                self.0.push((
                    url.to_owned(),
                    headers
                        .iter()
                        .map(|(k, v)| ((*k).to_owned(), v.clone()))
                        .collect(),
                ));
                Err("stop".into())
            }
            fn now_ms(&self) -> u64 {
                0
            }
            fn kv_get(&mut self, _: &str) -> Result<Option<String>, String> {
                Ok(None)
            }
            fn kv_set(&mut self, _: &str, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn kv_delete(&mut self, _: &str) {}
            fn kv_list(&mut self, _: &str) -> Result<Vec<String>, String> {
                Ok(vec![])
            }
            fn put_entry(&mut self, _: &Entry) -> Result<Put, String> {
                Err("no".into())
            }
            fn list_managed(&mut self) -> Result<Vec<Managed>, String> {
                Ok(vec![])
            }
            fn delete_entry(&mut self, _: &str) -> Result<(), String> {
                Ok(())
            }
        }
        let mut c = cfg();
        c.token = Some("tok".into());
        let mut spy = Spy(Vec::new());
        let _ = run(
            &mut spy,
            &c,
            None,
            Budget {
                deadline_ms: u64::MAX,
                max_requests: 5,
                max_writes: 5,
            },
        );
        let (url, headers) = &spy.0[0];
        assert!(url.starts_with("https://api.github.com/"));
        assert!(headers.contains(&("authorization".to_owned(), "Bearer tok".to_owned())));
    }
}
