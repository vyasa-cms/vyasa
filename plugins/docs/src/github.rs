//! The pieces of GitHub's HTTP API the importer uses, as pure functions.
//!
//! Two endpoints only: the git trees API (a listing with a content hash per
//! file, one call per folder level) and `raw.githubusercontent.com` (file
//! text at an immutable commit). No clone, no tarball: the host's `fetch`
//! is HTTPS GET, text only, redirects refused, 1 MiB per response.

/// `owner/name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// Account or organisation.
    pub owner: String,
    /// Repository name.
    pub name: String,
}

/// Parses `https://github.com/owner/name`, with optional `.git`, trailing
/// slash or `/tree/...` suffix.
///
/// # Errors
/// A message naming what is wrong, for the settings form.
pub fn parse_repo(url: &str) -> Result<Repo, String> {
    let rest = url
        .trim()
        .strip_prefix("https://github.com/")
        .ok_or_else(|| "repository must look like https://github.com/owner/name".to_owned())?;
    let mut parts = rest.split('/').filter(|s| !s.is_empty());
    let owner = parts.next().unwrap_or("");
    let name = parts.next().unwrap_or("").trim_end_matches(".git");
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !ok(owner) || !ok(name) || owner.starts_with('.') || name.starts_with('.') {
        return Err("repository must look like https://github.com/owner/name".to_owned());
    }
    Ok(Repo {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

/// Cleans a folder setting into path segments: no empty, `.` or `..`.
///
/// # Errors
/// When a segment tries to leave the repository.
pub fn folder_segments(folder: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for seg in folder
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != ".")
    {
        if seg == ".." || seg.contains('\\') {
            return Err("folder may not contain `..`".to_owned());
        }
        out.push(seg.to_owned());
    }
    Ok(out)
}

fn encode_segment(seg: &str) -> String {
    let mut out = String::new();
    for b in seg.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Percent-encodes each segment of a repository path.
#[must_use]
pub fn encode_path(path: &str) -> String {
    path.split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// `GET /repos/{o}/{r}/git/trees/{ref}` (a branch name or a tree sha).
#[must_use]
pub fn tree_url(repo: &Repo, tree_ref: &str, recursive: bool) -> String {
    format!(
        "https://api.github.com/repos/{}/{}/git/trees/{}{}",
        repo.owner,
        repo.name,
        encode_segment(tree_ref),
        if recursive { "?recursive=1" } else { "" }
    )
}

/// A file at a commit.
#[must_use]
pub fn raw_url(repo: &Repo, commit: &str, path: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        repo.owner,
        repo.name,
        commit,
        encode_path(path)
    )
}

/// `https://raw.githubusercontent.com/o/r/<commit>/` for rewriting images.
#[must_use]
pub fn raw_base(repo: &Repo, commit: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/{}/{}/{}/",
        repo.owner, repo.name, commit
    )
}

/// `https://github.com/o/r/blob/<commit>/` for rewriting links to files.
#[must_use]
pub fn blob_base(repo: &Repo, commit: &str) -> String {
    format!(
        "https://github.com/{}/{}/blob/{}/",
        repo.owner, repo.name, commit
    )
}

/// The "edit on GitHub" address of a file on a branch.
#[must_use]
pub fn edit_url(repo: &Repo, branch: &str, path: &str) -> String {
    format!(
        "https://github.com/{}/{}/edit/{}/{}",
        repo.owner,
        repo.name,
        encode_path(branch),
        encode_path(path)
    )
}

/// One entry of a git tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// Path below the tree that was listed.
    pub path: String,
    /// `blob` or `tree`.
    pub is_dir: bool,
    /// Git object hash: changes exactly when the content does.
    pub sha: String,
    /// Size in bytes (blobs).
    pub size: u64,
}

/// A parsed tree listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    /// This tree's own hash.
    pub sha: String,
    /// GitHub cut the listing short.
    pub truncated: bool,
    /// Entries.
    pub entries: Vec<TreeEntry>,
}

/// Parses the trees API response.
///
/// # Errors
/// When the body is not the expected JSON.
pub fn parse_tree(json: &str) -> Result<Tree, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    let sha = v["sha"].as_str().ok_or("tree has no sha")?.to_owned();
    let list = v["tree"].as_array().ok_or("tree has no entries")?;
    let entries = list
        .iter()
        .filter_map(|e| {
            let kind = e["type"].as_str()?;
            if kind != "blob" && kind != "tree" {
                return None; // submodule commits and the like
            }
            Some(TreeEntry {
                path: e["path"].as_str()?.to_owned(),
                is_dir: kind == "tree",
                sha: e["sha"].as_str()?.to_owned(),
                size: e["size"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    Ok(Tree {
        sha,
        truncated: v["truncated"].as_bool().unwrap_or(false),
        entries,
    })
}

/// The Markdown and `_meta.json` files of a recursive listing, below the
/// size cap, in path order.
#[must_use]
pub fn doc_files(tree: &Tree, max_bytes: u64) -> Vec<&TreeEntry> {
    let mut files: Vec<&TreeEntry> = tree
        .entries
        .iter()
        .filter(|e| {
            !e.is_dir
                && e.size <= max_bytes
                && !e
                    .path
                    .split('/')
                    .any(|s| s.starts_with('.') || s == "node_modules")
                && {
                    let lower = e.path.to_ascii_lowercase();
                    lower.ends_with(".md")
                        || lower.ends_with(".mdx")
                        || lower.ends_with("/_meta.json")
                        || lower == "_meta.json"
                }
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_parse_and_bad_ones_are_refused() {
        assert_eq!(
            parse_repo("https://github.com/vyasa-cms/vyasa").unwrap(),
            Repo {
                owner: "vyasa-cms".into(),
                name: "vyasa".into()
            }
        );
        assert_eq!(parse_repo("https://github.com/o/r.git/").unwrap().name, "r");
        assert_eq!(
            parse_repo("https://github.com/o/r/tree/main/docs")
                .unwrap()
                .name,
            "r"
        );
        for bad in [
            "http://github.com/o/r",
            "https://gitlab.com/o/r",
            "https://github.com/o",
            "https://github.com/../r",
            "https://github.com/o/r s",
            "ssh://git@github.com/o/r",
            "",
        ] {
            assert!(parse_repo(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn folders_clean_up_and_refuse_escapes() {
        assert_eq!(folder_segments("/docs/").unwrap(), ["docs"]);
        assert_eq!(folder_segments("./a//b").unwrap(), ["a", "b"]);
        assert!(folder_segments("a/../b").is_err());
        assert!(folder_segments("").unwrap().is_empty());
    }

    #[test]
    fn urls_are_built_from_checked_parts_and_encoded() {
        let r = Repo {
            owner: "o".into(),
            name: "r".into(),
        };
        assert_eq!(
            tree_url(&r, "feature/x", true),
            "https://api.github.com/repos/o/r/git/trees/feature%2Fx?recursive=1"
        );
        assert_eq!(
            raw_url(&r, "abc123", "docs/a b/é.md"),
            "https://raw.githubusercontent.com/o/r/abc123/docs/a%20b/%C3%A9.md"
        );
        assert_eq!(
            edit_url(&r, "main", "docs/x.md"),
            "https://github.com/o/r/edit/main/docs/x.md"
        );
        assert_eq!(blob_base(&r, "abc"), "https://github.com/o/r/blob/abc/");
    }

    #[test]
    fn a_listing_parses_and_filters_to_documentation() {
        let json = r#"{"sha":"t1","truncated":false,"tree":[
          {"path":"a.md","type":"blob","sha":"s1","size":10},
          {"path":"guides","type":"tree","sha":"s2"},
          {"path":"guides/b.mdx","type":"blob","sha":"s3","size":20},
          {"path":"guides/_meta.json","type":"blob","sha":"s4","size":5},
          {"path":"guides/img.png","type":"blob","sha":"s5","size":5},
          {"path":".github/x.md","type":"blob","sha":"s6","size":5},
          {"path":"big.md","type":"blob","sha":"s7","size":9999999},
          {"path":"sub","type":"commit","sha":"s8"}]}"#;
        let t = parse_tree(json).unwrap();
        assert_eq!(t.sha, "t1");
        assert_eq!(t.entries.len(), 7);
        let files: Vec<_> = doc_files(&t, 512 * 1024)
            .iter()
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(files, ["a.md", "guides/_meta.json", "guides/b.mdx"]);
        assert!(parse_tree("{}").is_err());
        assert!(parse_tree("<html>").is_err());
    }
}
