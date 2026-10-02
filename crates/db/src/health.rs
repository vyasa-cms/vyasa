//! Database health reporting.

use sqlx::PgPool;

/// Result of a database health probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthReport {
    /// Whether the database answered `SELECT 1`.
    pub ok: bool,
    /// Highest applied migration version; `None` when the migrations
    /// table does not exist yet (migrations never applied) or the probe
    /// could not read it.
    pub migration_version: Option<i64>,
    /// Failure description when `ok` is false.
    pub error: Option<String>,
}

/// Probes connectivity and the applied migration version.
///
/// This never panics and never returns an error: a dead database is a
/// health report with `ok: false`, not an exceptional condition.
pub async fn health(pool: &PgPool) -> HealthReport {
    let ok = match sqlx::query("SELECT 1").execute(pool).await {
        Ok(_) => true,
        Err(err) => {
            return HealthReport {
                ok: false,
                migration_version: None,
                error: Some(format!("connectivity check failed: {err}")),
            };
        }
    };
    let migration_version =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(pool)
            .await
            .ok();
    HealthReport {
        ok,
        migration_version,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::HealthReport;

    #[test]
    fn health_report_is_constructible() {
        let report = HealthReport {
            ok: true,
            migration_version: Some(0),
            error: None,
        };
        assert!(report.ok);
        assert_eq!(report.migration_version, Some(0));
    }
}
