-- 0025_plugin_runtime: storage and scheduling for plugins.
--
-- plugin_kv is a plugin's own private namespace. It is deliberately
-- separate from plugin_settings: settings are written by an operator and
-- read by the plugin, this is written by the plugin and never rendered as
-- configuration. Keeping them apart means a plugin cannot rewrite its own
-- declared configuration, and an operator's settings cannot be flooded out
-- by plugin state.
CREATE TABLE plugin_kv (
    plugin_id  BIGINT NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    value      TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (plugin_id, key)
);

-- Declared periodic work. The row survives restarts so a plugin scheduled
-- every six hours is not re-run every time the server is deployed.
CREATE TABLE plugin_tasks (
    plugin_id     BIGINT NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    every_seconds INTEGER NOT NULL CHECK (every_seconds >= 60),
    last_run_at   TIMESTAMPTZ,
    last_status   TEXT NOT NULL DEFAULT 'pending'
                  CONSTRAINT plugin_tasks_status_check
                  CHECK (last_status IN ('pending', 'ok', 'failed')),
    last_error    TEXT,
    PRIMARY KEY (plugin_id, name)
);

-- Posts a plugin created, so `update-post` can refuse to touch anything a
-- human wrote. A plugin with db:write:posts is not an editor account.
CREATE TABLE plugin_posts (
    post_id   BIGINT PRIMARY KEY REFERENCES posts (id) ON DELETE CASCADE,
    plugin_id BIGINT NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX plugin_posts_plugin_idx ON plugin_posts (plugin_id);
