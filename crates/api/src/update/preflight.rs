//! Everything checked *before* an upgrade moves a byte.
//!
//! Themes and plugins are database rows here, so a binary swap cannot
//! clobber them — the state that genuinely can be lost is on disk:
//! uploads, and the search index. The rest of this module is about the
//! two ways an upgrade goes wrong anyway: state that lives inside the
//! directory being replaced, and installed plugins compiled against a
//! contract the new build no longer speaks.

use std::path::{Path, PathBuf};

use serde::Serialize;
use vyasa_core::health::Status;

use super::manifest::{self, Release};
use super::mode::{DeploymentMode, Environment};
use crate::state::AppState;

/// One preflight finding.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Finding {
    /// Stable machine name.
    pub name: String,
    /// `ok` / `warn` / `fail`.
    #[schema(value_type = String)]
    pub status: Status,
    /// One line an operator can act on.
    pub detail: String,
}

impl Finding {
    fn new(name: &str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            status,
            detail: detail.into(),
        }
    }
    fn ok(name: &str, detail: impl Into<String>) -> Self {
        Self::new(name, Status::Ok, detail)
    }
    fn warn(name: &str, detail: impl Into<String>) -> Self {
        Self::new(name, Status::Warn, detail)
    }
    fn fail(name: &str, detail: impl Into<String>) -> Self {
        Self::new(name, Status::Fail, detail)
    }
}

/// The verdict of a whole preflight.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Preflight {
    /// Every check, in the order they were run.
    pub findings: Vec<Finding>,
    /// True when nothing failed — warnings do not block.
    pub can_proceed: bool,
}

impl Preflight {
    fn from(findings: Vec<Finding>) -> Self {
        let can_proceed = !findings.iter().any(|f| f.status == Status::Fail);
        Self {
            findings,
            can_proceed,
        }
    }
}

/// Whether `state_dir` sits inside `app_dir` — the arrangement that turns
/// "extract the new release over the old one" into "delete every upload".
fn nested_in(state_dir: &Path, app_dir: &Path) -> bool {
    let (Ok(state), Ok(app)) = (state_dir.canonicalize(), app_dir.canonicalize()) else {
        // Unresolvable paths are reported by their own check; do not
        // invent a nesting verdict from a guess.
        return false;
    };
    state.starts_with(&app) && state != app
}

/// Checks one on-disk state directory.
fn state_dir_finding(name: &str, dir: &Path, env: &Environment) -> Finding {
    if !dir.exists() {
        return Finding::warn(
            name,
            format!("{} does not exist yet; it will be created", dir.display()),
        );
    }
    let probe = dir.join(".vyasa-update-probe");
    if std::fs::write(&probe, b"").is_err() {
        return Finding::fail(name, format!("{} is not writable", dir.display()));
    }
    let _ = std::fs::remove_file(&probe);

    if env.mode == DeploymentMode::Docker {
        // In a container, uploads on the image filesystem are discarded
        // by the next `up`. This is the "my images vanished after the
        // upgrade" bug, catchable before rather than after.
        if !is_mount_point(dir) {
            return Finding::warn(
                name,
                format!(
                    "{} is on the container filesystem, not a volume — an image \
                     upgrade will discard it. Mount it before upgrading.",
                    dir.display()
                ),
            );
        }
        return Finding::ok(name, format!("{} is a mounted volume", dir.display()));
    }

    if let Some(binary) = env.binary_path.as_deref() {
        let app_dir = Path::new(binary).parent().unwrap_or(Path::new("/"));
        if nested_in(dir, app_dir) {
            return Finding::warn(
                name,
                format!(
                    "{} lives inside the directory holding the binary; replace the \
                     binary in place, never by extracting a release over the tree",
                    dir.display()
                ),
            );
        }
    }
    Finding::ok(
        name,
        format!("{} is writable and outside the app tree", dir.display()),
    )
}

/// True when `dir` is its own mount (a Docker volume), by comparing the
/// device of the directory with its parent's.
fn is_mount_point(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let Some(parent) = dir.parent() else {
            return true;
        };
        match (std::fs::metadata(dir), std::fs::metadata(parent)) {
            (Ok(here), Ok(up)) => here.dev() != up.dev(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        false
    }
}

/// Runs every check for an upgrade to `target` (or a general readiness
/// check when no release is named).
pub async fn run(state: &AppState, env: &Environment, target: Option<&Release>) -> Preflight {
    let mut out = Vec::new();

    // --- what the new build changes -----------------------------------
    if let Some(release) = target {
        if manifest::reachable_from(release, manifest::CURRENT_VERSION) {
            out.push(Finding::ok(
                "upgrade_path",
                format!(
                    "{} → {} is a supported step",
                    manifest::CURRENT_VERSION,
                    release.version
                ),
            ));
        } else {
            out.push(Finding::fail(
                "upgrade_path",
                format!(
                    "{} must be reached from {} or newer; upgrade in steps",
                    release.version,
                    release.min_upgrade_from.as_deref().unwrap_or("?")
                ),
            ));
        }
        if release.artifact_for_host().is_none() && env.mode.can_self_update() {
            out.push(Finding::fail(
                "artifact",
                format!(
                    "{} ships no build for {}",
                    release.version,
                    manifest::host_target()
                ),
            ));
        }
        if release.requires_attention {
            out.push(Finding::warn(
                "release_notes",
                "this release needs a manual step — read the notes before applying",
            ));
        }
    }

    // --- schema -------------------------------------------------------
    match vyasa_db::pending_migrations(&state.pool).await {
        Ok(pending) if pending.is_empty() => {
            out.push(Finding::ok("migrations", "no pending migrations"));
        }
        Ok(pending) => out.push(Finding::warn(
            "migrations",
            format!(
                "{} migration(s) will apply and cannot be undone: {}",
                pending.len(),
                pending
                    .iter()
                    .map(|(v, d)| format!("{v} {d}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
        Err(err) => out.push(Finding::fail(
            "migrations",
            format!("cannot read migration state: {err}"),
        )),
    }

    // --- the backup that is the only rollback -------------------------
    out.push(if which("pg_dump") {
        Finding::ok(
            "backup_tool",
            "pg_dump is available for the pre-upgrade backup",
        )
    } else {
        Finding::warn(
            "backup_tool",
            "pg_dump not found — migrations are forward-only, so without a \
             backup there is no way back",
        )
    });

    // --- state on disk ------------------------------------------------
    out.push(state_dir_finding("media", &state.config.media_dir, env));
    out.push(state_dir_finding(
        "search_index",
        &state.config.index_dir,
        env,
    ));

    // --- plugins ------------------------------------------------------
    out.push(plugin_finding(state, target).await);

    // --- can we act ---------------------------------------------------
    out.push(match env.mode {
        DeploymentMode::Docker | DeploymentMode::Kubernetes => Finding::ok(
            "deployment",
            format!(
                "{:?} deployment: the image is the unit of upgrade, so this \
                 install is upgraded from outside",
                env.mode
            ),
        ),
        _ if env.binary_replaceable => {
            Finding::ok("deployment", "the binary can be replaced in place")
        }
        _ => Finding::warn(
            "deployment",
            "the binary's directory is not writable; upgrade with the packaging \
             that installed it",
        ),
    });

    Preflight::from(out)
}

/// Installed plugins versus the contract the target release speaks.
async fn plugin_finding(state: &AppState, target: Option<&Release>) -> Finding {
    let installed = match state.plugins_repo.list().await {
        Ok(rows) => rows,
        Err(err) => {
            return Finding::warn("plugins", format!("cannot list plugins: {err}"));
        }
    };
    let enabled: Vec<&vyasa_db::repo::PluginRow> = installed.iter().filter(|p| p.enabled).collect();
    if enabled.is_empty() {
        return Finding::ok("plugins", "no plugins are enabled");
    }
    let contract_changes = target
        .and_then(|r| r.plugin_contract.as_deref())
        .is_some_and(|c| c != vyasa_plugins::CONTRACT_VERSION);
    if contract_changes {
        Finding::warn(
            "plugins",
            format!(
                "the plugin contract changes in this release; {} enabled plugin(s) \
                 must be rebuilt and will be disabled until they are: {}",
                enabled.len(),
                enabled
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    } else {
        Finding::ok(
            "plugins",
            format!(
                "{} enabled plugin(s) keep working; their code lives in the \
                 database, not in the release",
                enabled.len()
            ),
        )
    }
}

/// Whether `program` is on PATH.
fn which(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate: PathBuf = dir.join(program);
        candidate.is_file()
    })
}

#[cfg(test)]
mod tests {
    use super::{nested_in, Finding, Preflight};
    use vyasa_core::health::Status;

    #[test]
    fn a_failure_blocks_and_a_warning_does_not() {
        let warned = Preflight::from(vec![
            Finding::ok("a", "fine"),
            Finding::warn("b", "watch out"),
        ]);
        assert!(warned.can_proceed);
        let failed = Preflight::from(vec![Finding::warn("a", "x"), Finding::fail("b", "no")]);
        assert!(!failed.can_proceed);
        assert_eq!(failed.findings[1].status, Status::Fail);
    }

    #[test]
    fn state_inside_the_app_tree_is_recognised() {
        let root = tempfile::tempdir().expect("tempdir");
        let app = root.path().join("app");
        let inside = app.join("media");
        let outside = root.path().join("media");
        std::fs::create_dir_all(&inside).expect("inside");
        std::fs::create_dir_all(&outside).expect("outside");
        assert!(nested_in(&inside, &app));
        assert!(!nested_in(&outside, &app));
        assert!(!nested_in(&app, &app), "a directory is not inside itself");
    }
}
