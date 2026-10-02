-- 0018_plugin_settings: per-plugin settings kv (jsonb), brokered by
-- settings:read/write capabilities.

CREATE TABLE plugin_settings (
    plugin_id  BIGINT NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    value      JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (plugin_id, key)
);
