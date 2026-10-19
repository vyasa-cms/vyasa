//! The `vyasa-docs` component, enabled in a real state, importing a
//! repository served by an in-memory GitHub through `fetch-with`,
//! `put-entry` and `list-managed`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use serde_json::json;
use vyasa_plugins::host::{FetchBackend, FetchReply, HostEnv, V2Outcome};
use vyasa_testkit::TestDb;

use crate::authz::tests::test_state;
use crate::plugin_surface::Collect;

const DOCS_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/docs/docs-component.wasm"
);
const SHA1: &str = "1111111111111111111111111111111111111111";
const SHA2: &str = "2222222222222222222222222222222222222222";

type Files = BTreeMap<String, String>;

/// GitHub as the importer sees it: a branch head, git trees and raw files.
struct FakeGithub {
    commits: Mutex<BTreeMap<String, Files>>,
    head: Mutex<String>,
    requests: Mutex<Vec<String>>,
}

fn blob_sha(content: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut h);
    format!("{:040x}", h.finish())
}

fn tree_json(files: &Files, tree_ref: &str, recursive: bool) -> String {
    let prefix = tree_ref.strip_prefix("dir:").unwrap_or("");
    let mut entries = Vec::new();
    let mut dirs = BTreeSet::new();
    for (path, content) in files {
        let rest = if prefix.is_empty() {
            Some(path.as_str())
        } else {
            path.strip_prefix(&format!("{prefix}/"))
        };
        let Some(rest) = rest else { continue };
        if recursive {
            let parts: Vec<&str> = rest.split('/').collect();
            let mut acc = String::new();
            for seg in &parts[..parts.len() - 1] {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(seg);
                if dirs.insert(acc.clone()) {
                    let full = if prefix.is_empty() {
                        acc.clone()
                    } else {
                        format!("{prefix}/{acc}")
                    };
                    entries
                        .push(json!({"path": acc, "type": "tree", "sha": format!("dir:{full}")}));
                }
            }
            entries.push(json!({"path": rest, "type": "blob", "sha": blob_sha(content), "size": content.len()}));
        } else if let Some((d, _)) = rest.split_once('/') {
            if dirs.insert(d.to_owned()) {
                let full = if prefix.is_empty() {
                    d.to_owned()
                } else {
                    format!("{prefix}/{d}")
                };
                entries.push(json!({"path": d, "type": "tree", "sha": format!("dir:{full}")}));
            }
        } else {
            entries.push(json!({"path": rest, "type": "blob", "sha": blob_sha(content), "size": content.len()}));
        }
    }
    json!({"sha": tree_ref, "truncated": false, "tree": entries}).to_string()
}

impl FetchBackend for FakeGithub {
    fn get<'a>(
        &'a self,
        _url: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async { Err(String::from("the docs plugin uses fetch-with")) })
    }

    fn get_with<'a>(
        &'a self,
        url: &'a str,
        headers: Vec<(String, String)>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<FetchReply, String>> + Send + 'a>>
    {
        self.requests.lock().unwrap().push(url.to_owned());
        let head = self.head.lock().unwrap().clone();
        let reply = |status: u16, body: String, etag: Option<String>| FetchReply {
            status,
            headers: etag.map(|e| ("etag".to_owned(), e)).into_iter().collect(),
            body,
        };
        let out = if url == "https://api.github.com/repos/o/r/commits/main" {
            let etag = format!("\"{head}\"");
            if headers
                .iter()
                .any(|(k, v)| k == "if-none-match" && *v == etag)
            {
                reply(304, String::new(), None)
            } else {
                reply(200, head.clone(), Some(etag))
            }
        } else if let Some(rest) = url.strip_prefix("https://api.github.com/repos/o/r/git/trees/") {
            let (r, recursive) = rest
                .strip_suffix("?recursive=1")
                .map_or((rest, false), |r| (r, true));
            let files = self
                .commits
                .lock()
                .unwrap()
                .get(&head)
                .cloned()
                .unwrap_or_default();
            reply(
                200,
                tree_json(&files, &r.replace("%3A", ":"), recursive),
                None,
            )
        } else if let Some(rest) = url.strip_prefix("https://raw.githubusercontent.com/o/r/") {
            let (commit, path) = rest.split_once('/').unwrap();
            let body = self
                .commits
                .lock()
                .unwrap()
                .get(commit)
                .and_then(|f| f.get(path))
                .cloned();
            body.map_or_else(|| reply(404, String::new(), None), |b| reply(200, b, None))
        } else {
            return Box::pin(async move { Err(format!("unexpected url {url}")) });
        };
        Box::pin(async move { Ok(out) })
    }
}

fn repo_one() -> Files {
    let mut f = Files::new();
    f.insert("README.md".into(), "# the repo\n".into());
    f.insert(
        "docs/index.md".into(),
        "---\ntitle: Welcome\n---\nHello zebra, see [install](guides/install.md).\n".into(),
    );
    f.insert(
        "docs/guides/index.md".into(),
        "# Guides\n\nAll the guides.\n".into(),
    );
    f.insert(
        "docs/guides/install.md".into(),
        "---\ndescription: How to install\nsection: Start\n---\n# Install\n\n## Requirements\n\n<Callout type=\"warning\">Careful.</Callout>\n\n<Tabs items={[\"npm\",\"pnpm\"]}>\n<Tab>\n\n`npm i`\n\n</Tab>\n<Tab>\n\n`pnpm add`\n\n</Tab>\n</Tabs>\n\n## Next\n\nDone.\n".into(),
    );
    f.insert(
        "docs-v1/guides/install.md".into(),
        "# Install v1\n\nOld.\n".into(),
    );
    f
}

struct Site {
    _db: TestDb,
    state: crate::authz::tests::TestState,
    plugin_id: i64,
    github: Arc<FakeGithub>,
}

async fn site() -> Site {
    let db = TestDb::new().await;
    let state = test_state(&db);
    sqlx::query("INSERT INTO users (id, email, username, display_name, role, email_verified_at) VALUES (1, 'a@example.com', 'admin', 'Admin', 'admin', now())")
        .execute(&state.pool)
        .await
        .unwrap();
    let wasm = std::fs::read(DOCS_WASM).expect("run plugins/docs/build.sh");
    let manifest = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/docs/manifest.toml"
    ))
    .unwrap();
    let caps: serde_json::Value = toml_caps(&manifest);
    let repo = vyasa_db::repo::PluginsRepo::new(state.pool.clone());
    let row = repo
        .create(
            "vyasa-docs",
            "0.1.0",
            &wasm,
            &vyasa_plugins::host::sha256_hex(&wasm),
            &caps,
        )
        .await
        .unwrap();
    repo.set_enabled(row.id, true).await.unwrap();
    for (k, v) in [
        ("repo_url", json!("https://github.com/o/r")),
        ("branch", json!("main")),
        ("folder", json!("docs")),
        ("latest_version", json!("v2")),
        ("versions", json!("v1 = docs-v1")),
    ] {
        repo.setting_put(row.id, k, &v).await.unwrap();
    }
    let refused = crate::rest::plugins::refresh(&state, row.id, Collect::Enable).await;
    assert!(refused.is_none(), "{refused:?}");
    let github = Arc::new(FakeGithub {
        commits: Mutex::new(BTreeMap::from([(SHA1.to_owned(), repo_one())])),
        head: Mutex::new(SHA1.to_owned()),
        requests: Mutex::new(vec![]),
    });
    state.plugin_host.set_fetch_backend(github.clone());
    Site {
        _db: db,
        state,
        plugin_id: row.id,
        github,
    }
}

/// The capability list of the manifest, as the JSON array install stores.
fn toml_caps(manifest: &str) -> serde_json::Value {
    let mut caps = Vec::new();
    let mut inside = false;
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with("capabilities") {
            inside = true;
            continue;
        }
        if inside {
            if t == "]" {
                break;
            }
            caps.push(t.trim_matches(|c| c == '"' || c == ',').to_owned());
        }
    }
    json!(caps)
}

async fn run_sync(s: &Site) {
    let env = HostEnv::background(Arc::clone(&s.state.broker), s.state.pool.clone());
    match s
        .state
        .plugin_host
        .call_run_task_with(s.plugin_id, "sync", &env, true)
        .await
    {
        V2Outcome::Ok(()) => {}
        V2Outcome::Unsupported => panic!("the component does not implement run-task"),
        V2Outcome::Failed(reason) => panic!("sync failed: {reason}"),
    }
}

async fn paths(s: &Site) -> BTreeMap<String, (Option<String>, String)> {
    let rows: Vec<(String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT slug, path, status, title FROM posts WHERE type = 'doc' ORDER BY path",
    )
    .fetch_all(&s.state.pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|(slug, path, status, title)| (path.unwrap_or(slug), (Some(title), status)))
        .collect()
}

#[tokio::test]
async fn the_task_imports_a_repository_with_hierarchy_versions_and_blocks() {
    let s = site().await;
    run_sync(&s).await;
    let got = paths(&s).await;
    let keys: Vec<&str> = got.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "guides",
            "guides/install",
            "overview",
            "v1",
            "v1/guides",
            "v1/guides/install"
        ],
        "{got:?}"
    );
    assert!(got.values().all(|(_, status)| status == "published"));
    assert_eq!(got["overview"].0.as_deref(), Some("Welcome"));

    // Structure: parent links, positions, fields, terms, excerpt, edit URL.
    let (parent_slug, position, fields, excerpt): (Option<String>, i32, serde_json::Value, Option<String>) = sqlx::query_as(
        "SELECT (SELECT slug FROM posts p WHERE p.id = c.parent_id), position, meta->'fields', excerpt FROM posts c WHERE path = 'guides/install'",
    )
    .fetch_one(&s.state.pool)
    .await
    .unwrap();
    assert_eq!(parent_slug.as_deref(), Some("guides"));
    assert_eq!(position, 10);
    assert_eq!(fields["section_label"], "Start");
    assert!(
        fields["edit_url"]
            .as_str()
            .unwrap()
            .ends_with("/edit/main/docs/guides/install.md"),
        "{fields}"
    );
    assert_eq!(excerpt.as_deref(), Some("How to install"));
    let terms: Vec<String> = sqlx::query_scalar(
        "SELECT t.taxonomy || ':' || t.slug FROM term_relationships r JOIN terms t ON t.id = r.term_id JOIN posts p ON p.id = r.post_id WHERE p.path = 'v1/guides/install'",
    )
    .fetch_all(&s.state.pool)
    .await
    .unwrap();
    assert_eq!(terms, ["doc-version:v1"]);
    let root: serde_json::Value =
        sqlx::query_scalar("SELECT meta->'fields' FROM posts WHERE path = 'v1'")
            .fetch_one(&s.state.pool)
            .await
            .unwrap();
    assert_eq!(root["version_root"], true);

    // Blocks: the callout is core's, the tabs are the plugin's with two children,
    // links point at site paths.
    let content: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM posts WHERE path = 'guides/install'")
            .fetch_one(&s.state.pool)
            .await
            .unwrap();
    let kinds: Vec<&str> = content["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["heading", "callout", "docs/tabs", "heading", "paragraph"]
    );
    assert_eq!(
        content["blocks"][2]["children"].as_array().unwrap().len(),
        2
    );
    let home: serde_json::Value =
        sqlx::query_scalar("SELECT content FROM posts WHERE path = 'overview'")
            .fetch_one(&s.state.pool)
            .await
            .unwrap();
    assert!(
        home["blocks"][0]["attrs"]["text"]
            .as_str()
            .unwrap()
            .contains("href=\"/docs/guides/install\""),
        "{home}"
    );

    // The older version's page points search engines at its counterpart.
    let meta: serde_json::Value =
        sqlx::query_scalar("SELECT meta FROM posts WHERE path = 'v1/guides/install'")
            .fetch_one(&s.state.pool)
            .await
            .unwrap();
    assert_eq!(meta["seo_canonical_path"], "/docs/guides/install");

    // Every post is managed, read-only and owned.
    let managed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM managed_entries WHERE plugin_id = $1")
            .bind(s.plugin_id)
            .fetch_one(&s.state.pool)
            .await
            .unwrap();
    assert_eq!(managed, 6);
}

#[tokio::test]
async fn an_unchanged_branch_costs_one_request_and_a_change_rewrites_only_what_changed() {
    let s = site().await;
    run_sync(&s).await;
    let after_first = s.github.requests.lock().unwrap().len();
    run_sync(&s).await;
    let second: Vec<String> = s.github.requests.lock().unwrap()[after_first..].to_vec();
    // The first run was inside the poll interval of nothing, so it ran; the
    // second finds the interval not elapsed and does not even ask.
    assert!(second.len() <= 1, "{second:?}");

    // A new commit that edits one page; mark a check as due by pending.
    {
        let mut commits = s.github.commits.lock().unwrap();
        let mut files = commits[SHA1].clone();
        files.insert(
            "docs/guides/install.md".into(),
            "# Install\n\nChanged zebra body.\n".into(),
        );
        commits.insert(SHA2.to_owned(), files);
    }
    *s.github.head.lock().unwrap() = SHA2.to_owned();
    sqlx::query("INSERT INTO plugin_kv (plugin_id, key, value) VALUES ($1, 'sync/pending', $2) ON CONFLICT (plugin_id, key) DO UPDATE SET value = $2")
        .bind(s.plugin_id)
        .bind(SHA2)
        .execute(&s.state.pool)
        .await
        .unwrap();
    let updated_before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT updated_at FROM posts WHERE path = 'overview'")
            .fetch_one(&s.state.pool)
            .await
            .unwrap();
    run_sync(&s).await;
    let body: String =
        sqlx::query_scalar("SELECT content::text FROM posts WHERE path = 'guides/install'")
            .fetch_one(&s.state.pool)
            .await
            .unwrap();
    assert!(body.contains("Changed zebra body"), "{body}");
    let updated_after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT updated_at FROM posts WHERE path = 'overview'")
            .fetch_one(&s.state.pool)
            .await
            .unwrap();
    // The overview links to install, whose address did not change: untouched.
    assert_eq!(updated_before, updated_after);
}

#[tokio::test]
async fn a_removed_file_is_trashed_and_a_detached_page_is_left_alone() {
    let s = site().await;
    run_sync(&s).await;
    sqlx::query("UPDATE managed_entries SET detached = true WHERE post_id = (SELECT id FROM posts WHERE path = 'guides/install')")
        .execute(&s.state.pool)
        .await
        .unwrap();
    {
        let mut commits = s.github.commits.lock().unwrap();
        let mut files = commits[SHA1].clone();
        files.remove("docs-v1/guides/install.md");
        // The old version's folder must still exist; only one page left it.
        files.insert("docs-v1/keep.md".into(), "# Keep\n\nStays.\n".into());
        files.insert(
            "docs/guides/install.md".into(),
            "# Install\n\nUpstream rewrite.\n".into(),
        );
        commits.insert(SHA2.to_owned(), files);
    }
    *s.github.head.lock().unwrap() = SHA2.to_owned();
    sqlx::query("INSERT INTO plugin_kv (plugin_id, key, value) VALUES ($1, 'sync/pending', $2) ON CONFLICT (plugin_id, key) DO UPDATE SET value = $2")
        .bind(s.plugin_id)
        .bind(SHA2)
        .execute(&s.state.pool)
        .await
        .unwrap();
    run_sync(&s).await;
    let got = paths(&s).await;
    assert_eq!(got["v1/guides/install"].1, "trash", "{got:?}");
    assert!(!sqlx::query_scalar::<_, String>(
        "SELECT content::text FROM posts WHERE path = 'guides/install'"
    )
    .fetch_one(&s.state.pool)
    .await
    .unwrap()
    .contains("Upstream rewrite"));
}

#[tokio::test]
async fn a_bad_repository_url_fails_the_task_with_a_message_and_leaves_the_plugin_enabled() {
    let s = site().await;
    s.state
        .plugins_repo
        .setting_put(
            s.plugin_id,
            "repo_url",
            &json!("https://example.com/not/github"),
        )
        .await
        .unwrap();
    assert!(
        crate::rest::plugins::refresh(&s.state, s.plugin_id, Collect::Boot)
            .await
            .is_none()
    );
    let env = HostEnv::background(Arc::clone(&s.state.broker), s.state.pool.clone());
    match s
        .state
        .plugin_host
        .call_run_task_with(s.plugin_id, "sync", &env, true)
        .await
    {
        V2Outcome::Failed(reason) => assert!(reason.contains("Repository"), "{reason}"),
        other => panic!(
            "expected a failed task, got {}",
            match other {
                V2Outcome::Ok(()) => "ok",
                V2Outcome::Unsupported => "unsupported",
                V2Outcome::Failed(_) => "failed",
            }
        ),
    }
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM plugins WHERE id = $1")
        .bind(s.plugin_id)
        .fetch_one(&s.state.pool)
        .await
        .unwrap();
    assert!(
        enabled,
        "a typo in a setting never disables the plugin's blocks and sections"
    );
}
