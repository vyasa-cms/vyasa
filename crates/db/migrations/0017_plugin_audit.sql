-- 0017_plugin_audit: security-relevant broker decisions + fetch log.

CREATE TABLE plugin_audit (
    id         BIGINT PRIMARY KEY,
    plugin_id  BIGINT NOT NULL,
    ts         TIMESTAMPTZ NOT NULL DEFAULT now(),
    kind       TEXT NOT NULL CHECK (kind IN ('deny', 'fetch', 'quota')),
    capability TEXT NOT NULL,
    detail     TEXT NOT NULL DEFAULT ''
);

CREATE INDEX plugin_audit_plugin_ts_idx ON plugin_audit (plugin_id, ts DESC);
