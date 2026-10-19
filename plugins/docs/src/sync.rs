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

/// Bump to rewrite every entry after an importer change.
pub const FORMAT: &str = "f1";
/// Largest page the importer reads.
pub const MAX_PAGE_BYTES: u64 = 512 * 1024;
/// Page text kept between the scan and the write of one run.
const MAX_KEPT_BODIES: usize = 8 * 1024 * 1024;
/// Most pages one site may import.
pub const MAX_PAGES: usize = 2000;
/// The post type written.
pub const POST_TYPE: &str = "doc";
/// The URL base of the type.
pub const BASE: &str = "docs";
/// The version taxonomy.
pub const VERSION_TAXONOMY: &str = "doc-version";

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
    fn kv_get(&mut self, key: &str) -> Option<String>;
    /// Stores a value.
    ///
    /// # Errors
    /// Quota or size.
    fn kv_set(&mut self, key: &str, value: &str) -> Result<(), String>;
    /// Removes a value.
    fn kv_delete(&mut self, key: &str);
    /// Keys under a prefix.
    fn kv_list(&mut self, prefix: &str) -> Vec<String>;
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
    /// Nothing changed upstream since the last finished run.
    pub up_to_date: bool,
    /// The commit synced.
    pub commit: String,
    /// Notes for the operator.
    pub warnings: Vec<String>,
}

struct Ctx<'a, E: Env> {
    env: &'a mut E,
    cfg: &'a Config,
    budget: Budget,
    report: Report,
}

enum Stop {
    Budget,
    Fail(String),
}

impl From<String> for Stop {
    fn from(s: String) -> Self {
        Stop::Fail(s)
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
        Err(Stop::Fail(m)) => Err(m),
    }
}

fn sync<E: Env>(cx: &mut Ctx<E>, hint: Option<&str>) -> Result<(), Stop> {
    // 1. Which commit? A conditional request: unchanged costs nothing.
    let commit = match hint {
        Some(h) if h.len() >= 40 => h.to_owned(),
        _ => {
            let url = format!(
                "https://api.github.com/repos/{}/{}/commits/{}",
                cx.cfg.repo.owner,
                cx.cfg.repo.name,
                github::encode_path(&cx.cfg.branch)
            );
            let etag = cx.env.kv_get("sync/commit-etag");
            let done = cx.env.kv_get("sync/complete").as_deref() == Some("1");
            let r = cx.get(
                &url,
                "application/vnd.github.sha",
                if done { etag.as_deref() } else { None },
            )?;
            if r.status == 304 {
                cx.report.up_to_date = true;
                return Ok(());
            }
            if let Some(e) = &r.etag {
                let _ = cx.env.kv_set("sync/commit-etag-next", e);
            }
            r.body.trim().to_owned()
        }
    };
    if commit.len() < 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Stop::Fail("GitHub did not return a commit hash".to_owned()));
    }
    cx.report.commit.clone_from(&commit);
    if cx.env.kv_get("sync/commit").as_deref() != Some(commit.as_str()) {
        cx.env.kv_set("sync/commit", &commit)?;
        cx.env.kv_set("sync/complete", "0")?;
    }

    // 2. Files of every tree.
    let mut trees: Vec<TreeSpec> = vec![TreeSpec {
        label: cx.cfg.latest_version.clone(),
        folder: cx.cfg.folder.clone(),
        tag: "main".to_owned(),
    }];
    for (label, folder) in &cx.cfg.versions {
        trees.push(TreeSpec {
            label: label.clone(),
            folder: folder.clone(),
            tag: format!("v:{label}"),
        });
    }
    let mut files: Vec<SrcFile> = Vec::new();
    for t in &trees {
        let listing = list_folder(cx, &commit, &t.folder)?;
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
    for f in &files {
        let is_meta = f.path == "_meta.json" || f.path.ends_with("/_meta.json");
        let text_key = format!("scan/{}", f.sha);
        let cached = cx.env.kv_get(&text_key);
        let text = if let Some(c) = &cached {
            c.clone()
        } else {
            let url = github::raw_url(
                &cx.cfg.repo,
                &commit,
                &join_repo_path(&f.tree.folder, &f.path),
            );
            let body = cx.get(&url, "text/plain", None)?.body;
            if is_meta {
                // `_meta.json` is cached as it is; a value the key-value
                // store would refuse (64 KiB) is simply read again next time.
                if body.len() <= 60_000 {
                    let _ = cx.env.kv_set(&text_key, &body);
                }
                body
            } else {
                let scan = scan_page(&body);
                let json = scan.to_json();
                // Best effort: a full quota only makes the next run re-read this page.
                let _ = cx.env.kv_set(&text_key, &json);
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
    let today = civil_date(cx.env.now_ms());
    for pe in &p.entries {
        let hash_basis = match &pe.source {
            Source::Page(f) => format!("{FORMAT}:{}:{sig}", f.sha),
            Source::Generated => format!("{FORMAT}:{}", generated_basis(pe, &p.entries)),
        };
        let hash = hex(&Sha256::digest(hash_basis.as_bytes()));
        if managed.get(&pe.planned.key) == Some(&hash) {
            cx.report.unchanged += 1;
            continue;
        }
        if cx.report.created + cx.report.updated >= cx.budget.max_writes {
            return Err(Stop::Budget);
        }
        let entry = match &pe.source {
            Source::Page(f) => {
                let url = github::raw_url(
                    &cx.cfg.repo,
                    &commit,
                    &join_repo_path(&f.tree.folder, &f.path),
                );
                let body = match bodies.remove(&f.sha) {
                    Some(b) => b,
                    None => cx.get(&url, "text/plain", None)?.body,
                };
                build_page_entry(cx.cfg, &commit, f, &body, pe, &p, &hash, &today, &infos)
            }
            Source::Generated => build_generated_entry(pe, &p, &hash),
        };
        match cx.env.put_entry(&entry)? {
            Put::Created => cx.report.created += 1,
            Put::Updated => cx.report.updated += 1,
            Put::Unchanged => cx.report.unchanged += 1,
        }
    }

    // 6. Trash what the repository no longer has.
    let wanted: BTreeSet<&str> = p.entries.iter().map(|e| e.planned.key.as_str()).collect();
    for key in managed.keys() {
        if !wanted.contains(key.as_str()) {
            cx.env.delete_entry(key)?;
            cx.report.deleted += 1;
        }
    }

    // 7. Done: remember it, and forget what is no longer in the repository.
    // The per-blob scans stay: they are what lets the next run plan without
    // reading a page that has not changed.
    let live: BTreeSet<String> = files.iter().map(|f| format!("scan/{}", f.sha)).collect();
    for k in cx.env.kv_list("scan/") {
        if !live.contains(&k) {
            cx.env.kv_delete(&k);
        }
    }
    if let Some(next) = cx.env.kv_get("sync/commit-etag-next") {
        cx.env.kv_set("sync/commit-etag", &next)?;
    }
    cx.env.kv_set("sync/complete", "1")?;
    cx.env
        .kv_set("sync/last-ok-ms", &cx.env.now_ms().to_string())?;
    Ok(())
}

#[derive(Clone)]
struct TreeSpec {
    label: String,
    folder: Vec<String>,
    tag: String,
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
    slug: Option<String>,
    position: Option<i64>,
    hide: bool,
    version: Option<String>,
    section: Option<String>,
    updated: Option<String>,
}

fn scan_page(src: &str) -> Scan {
    let split = frontmatter::split(src);
    let h1 = blocks::convert(split.body, &Links::default()).h1;
    Scan {
        title: split.front.title.clone().or(h1).unwrap_or_default(),
        description: split.front.description.clone().unwrap_or_default(),
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
            "title": self.title, "description": self.description, "slug": self.slug, "position": self.position,
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
}

struct FullPlan {
    entries: Vec<PlannedEntry>,
    warnings: Vec<String>,
    /// Page key to site path (links).
    known: BTreeMap<(String, String), String>,
}

fn effective_version(cfg: &Config, f: &SrcFile, scan: &Scan) -> String {
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
    let mut entries = Vec::new();
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
        let is_latest = version == cfg.latest_version;
        let root_key = format!("@version/{version}");
        if !is_latest {
            roots_made += 1;
            entries.push(PlannedEntry {
                planned: Planned {
                    key: root_key.clone(),
                    parent_key: None,
                    slug: plan::slugify(&version),
                    title: version.clone(),
                    position: 1000 + roots_made * 10,
                    hide: true,
                    section: None,
                    kind: Kind::Group,
                    site_path: format!("/{BASE}/{}", plan::slugify(&version)),
                },
                source: Source::Generated,
                version: version.clone(),
                kind_version_root: true,
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
                vec![plan::slugify(&version)]
            },
            parent_key: if is_latest {
                None
            } else {
                Some(root_key.clone())
            },
            tree: if is_latest {
                "main".to_owned()
            } else {
                plan::slugify(&version)
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

fn generated_basis(pe: &PlannedEntry, all: &[PlannedEntry]) -> String {
    let mut s = format!(
        "{}|{}|{}|{}|{}",
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
        s.push_str(&format!("|{}={}", c.planned.site_path, c.planned.title));
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
    Ok(t)
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
    if version.is_empty() {
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
        &cfg.branch,
        &join_repo_path(&f.tree.folder, &f.path),
    );
    // An older version's page points search engines at its counterpart.
    let canonical = (pe.version != cfg.latest_version)
        .then(|| {
            let rel = pe.planned.site_path.splitn(4, '/').nth(3).unwrap_or("");
            let latest = format!("/{BASE}/{rel}");
            p.entries
                .iter()
                .any(|e| e.planned.site_path == latest && e.version == cfg.latest_version)
                .then_some(latest)
        })
        .flatten();
    let seo = canonical.map_or_else(
        || "{}".to_owned(),
        |c| serde_json::json!({ "canonical-path": c }).to_string(),
    );
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

fn build_generated_entry(pe: &PlannedEntry, p: &FullPlan, hash: &str) -> Entry {
    let mut items = Vec::new();
    for c in p.entries.iter().filter(|c| {
        c.planned.parent_key.as_deref() == Some(pe.planned.key.as_str()) && !c.planned.hide
    }) {
        items.push(serde_json::json!({"kind": "paragraph", "attrs": {"text": format!("<a href=\"{}\">{}</a>", blocks::esc(&c.planned.site_path), blocks::esc(&c.planned.title))}, "children": []}));
    }
    let intro = if pe.kind_version_root {
        format!("Documentation for {}.", blocks::esc(&pe.planned.title))
    } else {
        format!("In this section: {}.", blocks::esc(&pe.planned.title))
    };
    let mut blocks =
        vec![serde_json::json!({"kind": "paragraph", "attrs": {"text": intro}, "children": []})];
    if !items.is_empty() {
        blocks.push(
            serde_json::json!({"kind": "list", "attrs": {"ordered": false}, "children": items}),
        );
    }
    Entry {
        key: pe.planned.key.clone(),
        parent_key: pe.planned.parent_key.clone(),
        slug: pe.planned.slug.clone(),
        title: pe.planned.title.clone(),
        excerpt: String::new(),
        blocks_json: serde_json::Value::Array(blocks).to_string(),
        position: pe.planned.position,
        terms: terms_for(&pe.version),
        fields_json: doc_fields(pe.kind_version_root, pe.planned.hide, "", "", ""),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const SHA1: &str = "1111111111111111111111111111111111111111";
    const SHA2: &str = "2222222222222222222222222222222222222222";

    type Files = BTreeMap<String, String>;

    #[derive(Default)]
    struct Fake {
        commits: BTreeMap<String, Files>, // commit -> files (full repo paths)
        head: String,
        kv: RefCell<BTreeMap<String, String>>,
        entries: BTreeMap<String, (Entry, String)>,
        now: u64,
        requests: Vec<String>,
        etag_seen: Vec<Option<String>>,
        fail_status: Option<u16>,
        step_ms: u64,
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
                let etag = format!("\"{}\"", f.head);
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
                    body: f.head.clone(),
                });
            }
            if let Some(rest) = url.strip_prefix("https://api.github.com/repos/o/r/git/trees/") {
                let (r, recursive) = rest
                    .strip_suffix("?recursive=1")
                    .map_or((rest, false), |r| (r, true));
                let tree_ref = r.replace("%3A", ":");
                let commit = f.head.clone();
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
        fn kv_get(&mut self, key: &str) -> Option<String> {
            self.0.borrow().kv.borrow().get(key).cloned()
        }
        fn kv_set(&mut self, key: &str, value: &str) -> Result<(), String> {
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
        fn kv_list(&mut self, prefix: &str) -> Vec<String> {
            self.0
                .borrow()
                .kv
                .borrow()
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect()
        }
        fn put_entry(&mut self, e: &Entry) -> Result<Put, String> {
            let mut f = self.0.borrow_mut();
            if let Some(parent) = &e.parent_key {
                if !f.entries.contains_key(parent) {
                    return Err(format!("parent {parent} has not been written"));
                }
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
        assert_eq!(report.updated, 1, "{report:?}");
        assert_eq!(report.created, 0);
        assert!(report.unchanged >= 2);
        // intro links to install; its own text did not change, nor did any path.
        assert!(f.borrow().entries["main:guides/install.md"]
            .0
            .blocks_json
            .contains("Changed body"));
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
        let count = |f: &RefCell<Fake>| {
            f.borrow()
                .kv
                .borrow()
                .keys()
                .filter(|k| k.starts_with("scan/"))
                .count()
        };
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
        let cached = f
            .borrow()
            .kv
            .borrow()
            .keys()
            .filter(|k| k.starts_with("scan/"))
            .count();
        assert!(cached >= 1, "pages read so far are remembered");
        let b = budget(&f);
        let second = run(&mut Wrap(&f), &cfg(), None, b).unwrap();
        assert!(second.finished, "{second:?}");
        assert_eq!(f.borrow().entries.len(), 3);
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
            fn kv_get(&mut self, _: &str) -> Option<String> {
                None
            }
            fn kv_set(&mut self, _: &str, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn kv_delete(&mut self, _: &str) {}
            fn kv_list(&mut self, _: &str) -> Vec<String> {
                vec![]
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
