//! Performing an upgrade: download, verify, back up, swap, migrate,
//! restart, and prove the result works.
//!
//! The ordering is the whole design. Nothing irreversible happens until
//! everything reversible has succeeded: the download is verified before
//! it is staged, the database is dumped before a migration runs, and the
//! old binary is kept until the new one has answered a health check.
//!
//! Rollback is deliberately *scoped*. It fires only when the new build
//! fails to come up — the window in which nothing has been written and
//! the previous binary is still correct for the schema. Once the new
//! version has served a request, going back means restoring the dump,
//! because forward-only migrations plus `deny_unknown_fields` on the
//! token documents mean an older binary can no longer read what the new
//! one has written. Offering a button there would corrupt, not rescue.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vyasa_common::AppError;

use super::manifest::Artifact;

/// Where a run records its progress, beside the binary it replaces.
pub const STATE_FILE: &str = "vyasa-update.json";

/// Largest release artifact accepted: 512 MiB. A Vyasa binary is tens
/// of megabytes; this leaves room for growth and bundled assets while
/// keeping a bogus multi-gigabyte response from being buffered.
const MAX_ARTIFACT_BYTES: usize = 512 * 1024 * 1024;

/// How long the new build gets to answer before it is judged failed.
const HEALTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// One step of a run, as the admin polls it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Checking the ground.
    Preflight,
    /// Fetching the artifact.
    Downloading,
    /// Checksum and signature.
    Verifying,
    /// Dumping the database.
    BackingUp,
    /// Replacing the binary and the admin bundle.
    Swapping,
    /// Applying migrations.
    Migrating,
    /// Waiting for the new build to answer.
    Restarting,
    /// Finished successfully.
    Done,
    /// Failed; `detail` says where.
    Failed,
    /// Failed and the previous binary was put back.
    RolledBack,
}

/// The run's public state.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Progress {
    /// Version being installed.
    pub target: String,
    /// Where the run is.
    pub stage: Stage,
    /// One line for the operator.
    pub detail: String,
    /// Where the pre-upgrade dump was written, when one was taken.
    #[serde(default)]
    pub backup_path: Option<String>,
    /// When the run started (RFC 3339).
    pub started_at: String,
}

impl Progress {
    /// A fresh run record.
    #[must_use]
    pub fn new(target: &str, stage: Stage, detail: impl Into<String>) -> Self {
        Self {
            target: target.to_owned(),
            stage,
            detail: detail.into(),
            backup_path: None,
            started_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Writes the run state beside `binary`, best effort: losing the
    /// progress file must never abort an upgrade that is going fine.
    pub fn write_beside(&self, binary: &Path) {
        let Some(dir) = binary.parent() else { return };
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(dir.join(STATE_FILE), json);
        }
    }

    /// Reads the run state beside `binary`, if any.
    #[must_use]
    pub fn read_beside(binary: &Path) -> Option<Self> {
        let dir = binary.parent()?;
        let raw = std::fs::read_to_string(dir.join(STATE_FILE)).ok()?;
        serde_json::from_str(&raw).ok()
    }
}

/// Downloads `artifact`, checking its digest and (when the channel signs)
/// its signature before the bytes are used for anything.
///
/// # Errors
/// [`AppError::Validation`] when the digest or signature does not match —
/// the bytes are dropped, never staged.
pub async fn download(artifact: &Artifact, trusted_keys: &[String]) -> Result<Vec<u8>, AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| AppError::internal_msg(e.to_string()))?;
    let response = client
        .get(&artifact.url)
        .send()
        .await
        .map_err(|e| AppError::internal_msg(format!("download failed: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::internal_msg(format!("download failed: {e}")))?;
    // Counted while it streams, so a hostile or broken channel cannot
    // exhaust memory before the digest check ever runs.
    let bytes = crate::net_guard::read_capped(response, MAX_ARTIFACT_BYTES)
        .await
        .map_err(|e| match e {
            crate::net_guard::BodyError::TooLarge => AppError::validation(format!(
                "release artifact is larger than the {MAX_ARTIFACT_BYTES}-byte limit"
            )),
            crate::net_guard::BodyError::Transport(e) => {
                AppError::internal_msg(format!("download truncated: {e}"))
            }
        })?;
    verify_bytes(&bytes, artifact, trusted_keys)?;
    Ok(bytes)
}

/// Digest and signature check for a release artifact.
///
/// Delegates to [`crate::signing`], which is the one place that decides
/// whether downloaded bytes may be trusted — releases, marketplace
/// plugins and marketplace themes all answer to the same rules.
///
/// # Errors
/// [`AppError::Validation`] on any mismatch.
pub fn verify_bytes(
    bytes: &[u8],
    artifact: &Artifact,
    trusted_keys: &[String],
) -> Result<(), AppError> {
    crate::signing::verify_download(
        bytes,
        &artifact.sha256,
        artifact.signature.as_deref(),
        trusted_keys,
    )
}

/// Dumps the database next to the binary and returns the path.
///
/// This dump is the rollback plan — migrations are forward-only — so a
/// failure here stops the upgrade rather than being logged and ignored.
///
/// # Errors
/// [`AppError::Internal`] when `pg_dump` is missing or fails.
pub fn back_up(database_url: &str, dir: &Path, version: &str) -> Result<PathBuf, AppError> {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("vyasa-backup-{version}-{stamp}.sql"));
    let output = std::process::Command::new("pg_dump")
        .arg("--no-owner")
        .arg("--file")
        .arg(&path)
        .arg(database_url)
        .output()
        .map_err(|e| AppError::internal_msg(format!("pg_dump could not run: {e}")))?;
    if !output.status.success() {
        return Err(AppError::internal_msg(format!(
            "pg_dump failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(path)
}

/// Replaces `binary` with `new_bytes`, keeping the old one alongside.
///
/// The replacement is written into the *same directory* so the final
/// `rename` is atomic: a reader either sees the whole old file or the
/// whole new one, never a half-written binary. A staged file in `/tmp`
/// would cross filesystems and lose that guarantee.
///
/// # Errors
/// [`AppError::Internal`] on any filesystem failure; the original binary
/// is left in place unless the rename itself succeeded.
pub fn swap_binary(binary: &Path, new_bytes: &[u8]) -> Result<PathBuf, AppError> {
    let dir = binary
        .parent()
        .ok_or_else(|| AppError::internal_msg("binary has no parent directory"))?;
    let staged = dir.join(".vyasa-update-staged");
    let previous = dir.join("vyasa.previous");

    let mut file = std::fs::File::create(&staged)
        .map_err(|e| AppError::internal_msg(format!("cannot stage the new binary: {e}")))?;
    file.write_all(new_bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| AppError::internal_msg(format!("cannot write the new binary: {e}")))?;
    drop(file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| AppError::internal_msg(format!("cannot make it executable: {e}")))?;
    }

    // Keep the outgoing binary before anything replaces it: this copy is
    // what an immediate rollback restores.
    std::fs::copy(binary, &previous)
        .map_err(|e| AppError::internal_msg(format!("cannot keep the current binary: {e}")))?;
    std::fs::rename(&staged, binary)
        .map_err(|e| AppError::internal_msg(format!("cannot install the new binary: {e}")))?;
    Ok(previous)
}

/// Puts `previous` back. Only correct while the new build has not served
/// anything — see the module comment.
///
/// # Errors
/// [`AppError::Internal`] when the copy fails.
pub fn restore_binary(binary: &Path, previous: &Path) -> Result<(), AppError> {
    if !previous.exists() {
        return Err(AppError::internal_msg(
            "no previous binary was kept; restore from your own backup",
        ));
    }
    std::fs::copy(previous, binary)
        .map_err(|e| AppError::internal_msg(format!("cannot restore the previous binary: {e}")))?;
    Ok(())
}

/// Polls `{base}/api/v1/version` until it answers with `expected`, or the
/// timeout expires.
///
/// Connection refused is *not* a failure while the window is open: it is
/// the expected state of a process that is still starting.
pub async fn await_health(base: &str, expected: &str) -> Result<(), AppError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| AppError::internal_msg(e.to_string()))?;
    let deadline = std::time::Instant::now() + HEALTH_TIMEOUT;
    let mut last = String::from("no answer yet");
    while std::time::Instant::now() < deadline {
        match client.get(format!("{base}/api/v1/version")).send().await {
            Ok(response) => match response.json::<serde_json::Value>().await {
                Ok(body) => {
                    let served = body
                        .get("version")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    if served == expected {
                        return Ok(());
                    }
                    last = format!("still serving {served}");
                }
                Err(e) => last = format!("unreadable answer: {e}"),
            },
            Err(e) => last = format!("not up yet: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    }
    Err(AppError::internal_msg(format!(
        "the new build did not answer within {}s ({last})",
        HEALTH_TIMEOUT.as_secs()
    )))
}

/// Unpacks a release tarball into `staging` and returns the binary and,
/// when the release ships one, the admin bundle inside it.
///
/// Binary and bundle move together or not at all: the admin is
/// version-stamped against the server that serves it, and a mismatched
/// pair is the one failure that looks like data corruption to a user.
///
/// # Errors
/// [`AppError::Internal`] when extraction fails or no binary is found.
pub fn unpack_release(tarball: &[u8], staging: &Path) -> Result<Unpacked, AppError> {
    std::fs::create_dir_all(staging)
        .map_err(|e| AppError::internal_msg(format!("cannot create staging dir: {e}")))?;
    let archive = staging.join("release.tar.gz");
    std::fs::write(&archive, tarball)
        .map_err(|e| AppError::internal_msg(format!("cannot write the archive: {e}")))?;
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(staging)
        .status()
        .map_err(|e| AppError::internal_msg(format!("tar could not run: {e}")))?;
    if !status.success() {
        return Err(AppError::internal_msg("unpacking the release failed"));
    }
    let binary = find_file(staging, "vyasa")
        .ok_or_else(|| AppError::internal_msg("the release contains no `vyasa` binary"))?;
    let admin = find_dir(staging, "dist");
    Ok(Unpacked { binary, admin })
}

/// What a release tarball yielded.
pub struct Unpacked {
    /// The new binary.
    pub binary: PathBuf,
    /// The matching admin bundle, when the release ships one.
    pub admin: Option<PathBuf>,
}

/// Finds a file named `name` up to two levels down.
fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    let direct = root.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let candidate = entry.path().join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Finds a directory named `name` up to two levels down.
fn find_dir(root: &Path, name: &str) -> Option<PathBuf> {
    let direct = root.join(name);
    if direct.is_dir() {
        return Some(direct);
    }
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        let candidate = path.join(name);
        if candidate.is_dir() {
            return Some(candidate);
        }
        // admin/dist inside the tarball.
        let nested = path.join("admin").join(name);
        if nested.is_dir() {
            return Some(nested);
        }
    }
    None
}

/// Puts the new admin bundle in place, keeping the old one beside it.
///
/// # Errors
/// [`AppError::Internal`] when the swap fails; the previous bundle stays
/// usable because it is only renamed, never deleted.
pub fn swap_admin(new_dist: &Path, live_dist: &Path) -> Result<(), AppError> {
    let previous = live_dist.with_extension("previous");
    let _ = std::fs::remove_dir_all(&previous);
    if live_dist.exists() {
        std::fs::rename(live_dist, &previous)
            .map_err(|e| AppError::internal_msg(format!("cannot move the old bundle: {e}")))?;
    }
    if let Some(parent) = live_dist.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::rename(new_dist, live_dist) {
        Ok(()) => Ok(()),
        Err(err) => {
            // Put the old bundle back rather than leaving no admin at all.
            let _ = std::fs::rename(&previous, live_dist);
            Err(AppError::internal_msg(format!(
                "cannot install the new bundle: {err}"
            )))
        }
    }
}

/// Asks the running server to finish and exit, so a supervisor can start
/// the replacement. Returns false when the process is already gone.
///
/// SIGTERM, not SIGKILL: the server drains in-flight requests on that
/// signal, and the whole point of restarting this way is that nobody
/// mid-request sees the upgrade. Sent via `kill(1)` because this crate
/// forbids `unsafe`, and one process spawn on an upgrade is free.
#[must_use]
pub fn signal_restart(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::{restore_binary, swap_binary, verify_bytes, Artifact, Progress, Stage};

    fn artifact(sha: &str, signature: Option<&str>) -> Artifact {
        Artifact {
            platform: "x86_64-unknown-linux-gnu".into(),
            url: "https://example.test/v.tar.gz".into(),
            sha256: sha.into(),
            signature: signature.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn a_wrong_checksum_is_refused() {
        let bytes = b"the release";
        let good = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes));
        assert!(verify_bytes(bytes, &artifact(&good, None), &[]).is_ok());
        assert!(verify_bytes(bytes, &artifact(&"0".repeat(64), None), &[]).is_err());
    }

    #[test]
    fn signing_rules_cut_both_ways() {
        let bytes = b"the release";
        let good = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes));
        // Signed artifact, no key configured: refused rather than trusted.
        let signed = artifact(&good, Some(&"ab".repeat(64)));
        assert!(verify_bytes(bytes, &signed, &[]).is_err());
        // Unsigned artifact where keys are configured: also refused.
        let unsigned = artifact(&good, None);
        assert!(verify_bytes(bytes, &unsigned, &["aa".repeat(32)]).is_err());
        // A signature that matches no trusted key: refused.
        assert!(verify_bytes(bytes, &signed, &["aa".repeat(32)]).is_err());
    }

    #[test]
    fn a_real_signature_from_a_trusted_key_passes() {
        use ed25519_dalek::{Signer as _, SigningKey};
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let bytes = b"the release";
        let signature = hex::encode(key.sign(bytes).to_bytes());
        let public = hex::encode(key.verifying_key().to_bytes());
        let digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(bytes));
        assert!(verify_bytes(bytes, &artifact(&digest, Some(&signature)), &[public]).is_ok());
    }

    #[test]
    fn the_swap_keeps_the_outgoing_binary_and_can_undo_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let binary = dir.path().join("vyasa");
        std::fs::write(&binary, b"old build").expect("write");

        let previous = swap_binary(&binary, b"new build").expect("swap");
        assert_eq!(std::fs::read(&binary).expect("read"), b"new build");
        assert_eq!(std::fs::read(&previous).expect("kept"), b"old build");
        assert!(
            !dir.path().join(".vyasa-update-staged").exists(),
            "the staged file is renamed, not left behind"
        );

        restore_binary(&binary, &previous).expect("restore");
        assert_eq!(std::fs::read(&binary).expect("read back"), b"old build");
    }

    #[test]
    fn progress_round_trips_beside_the_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let binary = dir.path().join("vyasa");
        std::fs::write(&binary, b"x").expect("write");
        let mut progress = Progress::new("1.2.0", Stage::Migrating, "applying 2 migrations");
        progress.backup_path = Some("/tmp/dump.sql".into());
        progress.write_beside(&binary);

        let read = Progress::read_beside(&binary).expect("read back");
        assert_eq!(read.target, "1.2.0");
        assert_eq!(read.stage, Stage::Migrating);
        assert_eq!(read.backup_path.as_deref(), Some("/tmp/dump.sql"));
    }
}
