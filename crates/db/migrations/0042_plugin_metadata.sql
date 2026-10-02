-- 0042_plugin_metadata: what a plugin says about itself, and why it is
-- degraded when it is.

ALTER TABLE plugins
    ADD COLUMN description   TEXT NOT NULL DEFAULT '',
    ADD COLUMN author        TEXT NOT NULL DEFAULT '',
    ADD COLUMN homepage      TEXT NOT NULL DEFAULT '',
    ADD COLUMN license       TEXT NOT NULL DEFAULT '',
    ADD COLUMN status_reason TEXT NOT NULL DEFAULT '';
