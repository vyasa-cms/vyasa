-- 0011_plugins: installed WASM plugin registry with version history.

CREATE TABLE plugins (
    id         BIGINT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    version    TEXT NOT NULL,
    enabled    BOOLEAN NOT NULL DEFAULT false,
    status     TEXT NOT NULL DEFAULT 'installed'
               CONSTRAINT plugins_status_check
               CHECK (status IN ('installed', 'loaded', 'degraded', 'errored', 'disabled')),
    capabilities JSONB NOT NULL DEFAULT '[]'::jsonb,
    wasm_sha256  TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE plugin_versions (
    id         BIGINT PRIMARY KEY,
    plugin_id  BIGINT NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    version    TEXT NOT NULL,
    wasm       BYTEA NOT NULL,
    sha256     TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (plugin_id, version)
);

CREATE INDEX plugin_versions_plugin_idx ON plugin_versions (plugin_id);
