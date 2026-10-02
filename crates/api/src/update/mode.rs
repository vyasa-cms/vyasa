//! Where this install runs, and therefore what an upgrade may do to it.
//!
//! The whole update feature rests on one honest distinction: a process
//! that owns its own binary and can be restarted may replace itself; a
//! container cannot (the image is the unit, and anything written inside
//! it is discarded on the next `up`). Guessing wrong in either direction
//! is worse than not offering the button, so detection is conservative:
//! anything not provably self-managed is treated as managed-elsewhere.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// How this install is deployed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentMode {
    /// A binary this process can replace, restarted by a supervisor or by
    /// the updater itself.
    Standalone,
    /// systemd-managed binary: replaceable, restarted by systemd.
    Systemd,
    /// Inside a container. The image is the unit of upgrade.
    Docker,
    /// Inside Kubernetes. The deployment pipeline owns the rollout.
    Kubernetes,
}

impl DeploymentMode {
    /// Whether the updater may swap the binary here.
    #[must_use]
    pub const fn can_self_update(self) -> bool {
        matches!(self, Self::Standalone | Self::Systemd)
    }

    /// The commands an operator runs when we cannot act for them.
    #[must_use]
    pub fn instructions(self, version: &str) -> Vec<String> {
        match self {
            Self::Docker => vec![
                format!("docker compose pull   # or: docker pull ghcr.io/…/vyasa:{version}"),
                "docker compose run --rm app migrate".to_owned(),
                "docker compose up -d".to_owned(),
            ],
            Self::Kubernetes => vec![
                format!("kubectl set image deployment/vyasa vyasa=ghcr.io/…/vyasa:{version}"),
                "kubectl rollout status deployment/vyasa".to_owned(),
                "# run `vyasa migrate` as a Job before the rollout completes".to_owned(),
            ],
            Self::Standalone | Self::Systemd => vec![format!("vyasa update apply --to {version}")],
        }
    }
}

/// What the process can see about its own deployment.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Environment {
    /// Detected mode.
    pub mode: DeploymentMode,
    /// Path of the running binary, when it can be resolved.
    pub binary_path: Option<String>,
    /// Whether the binary's directory is writable — a swap needs to
    /// create the replacement beside it for `rename` to be atomic.
    pub binary_replaceable: bool,
    /// Whether a supervisor will restart the process when it exits.
    pub restart_supervised: bool,
}

/// True inside a container.
fn in_container() -> bool {
    if Path::new("/.dockerenv").exists() {
        return true;
    }
    std::fs::read_to_string("/proc/1/cgroup").is_ok_and(|cgroup| {
        cgroup.contains("docker") || cgroup.contains("containerd") || cgroup.contains("kubepods")
    })
}

/// True when systemd started this process (it sets `INVOCATION_ID` for
/// every unit it launches).
fn under_systemd() -> bool {
    std::env::var_os("INVOCATION_ID").is_some()
}

/// Whether `path`'s parent directory accepts a new file — the staged
/// binary must land on the same filesystem for the swap to be atomic.
fn parent_writable(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let probe = parent.join(".vyasa-update-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Inspects the running process.
#[must_use]
pub fn detect() -> Environment {
    let binary: Option<PathBuf> = std::env::current_exe().ok();
    let mode = if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() {
        DeploymentMode::Kubernetes
    } else if in_container() {
        DeploymentMode::Docker
    } else if under_systemd() {
        DeploymentMode::Systemd
    } else {
        DeploymentMode::Standalone
    };
    let replaceable = mode.can_self_update() && binary.as_deref().is_some_and(parent_writable);
    Environment {
        mode,
        binary_path: binary.as_ref().map(|p| p.display().to_string()),
        binary_replaceable: replaceable,
        // systemd restarts a unit that exits; a bare process needs the
        // updater to start its successor itself.
        restart_supervised: mode == DeploymentMode::Systemd,
    }
}

#[cfg(test)]
mod tests {
    use super::DeploymentMode;

    #[test]
    fn only_self_managed_modes_may_swap_a_binary() {
        assert!(DeploymentMode::Standalone.can_self_update());
        assert!(DeploymentMode::Systemd.can_self_update());
        assert!(!DeploymentMode::Docker.can_self_update());
        assert!(!DeploymentMode::Kubernetes.can_self_update());
    }

    #[test]
    fn managed_modes_hand_back_real_commands() {
        let docker = DeploymentMode::Docker.instructions("1.2.0");
        assert!(docker.iter().any(|l| l.contains("docker compose up -d")));
        assert!(
            docker.iter().any(|l| l.contains("migrate")),
            "migrations are not run by the entrypoint, so the steps must say so"
        );
        let k8s = DeploymentMode::Kubernetes.instructions("1.2.0");
        assert!(k8s.iter().any(|l| l.contains("1.2.0")));
    }
}
