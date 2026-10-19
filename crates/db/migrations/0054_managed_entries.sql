-- 0054_managed_entries: entries a plugin keeps in step with an outside
-- source (0.3).
--
-- One row per such entry. `ext_key` is the plugin's own identity for it
-- (a file path, a Notion page id) and is unique per plugin; `source_hash`
-- is what the plugin last wrote, so an unchanged source costs no write.
-- `detached` is a tombstone: an author who took a page over keeps it, and
-- the plugin's next sync finds the key detached and leaves the page alone.
--
-- Deleting the post or the plugin deletes the row; the entries themselves
-- survive their plugin, as every plugin-made entry does.
--
-- `content_fields.owner` records who defined a field: 'admin' for every
-- existing row, a plugin's name for fields its post types declare. The
-- fields API refuses to edit or delete a field it does not own.
CREATE TABLE managed_entries (
    post_id     BIGINT PRIMARY KEY REFERENCES posts (id) ON DELETE CASCADE,
    plugin_id   BIGINT NOT NULL REFERENCES plugins (id) ON DELETE CASCADE,
    ext_key     TEXT NOT NULL CHECK (char_length(ext_key) BETWEEN 1 AND 512),
    source_hash TEXT NOT NULL DEFAULT '',
    edit_url    TEXT NOT NULL DEFAULT '' CHECK (char_length(edit_url) <= 2048),
    detached    BOOLEAN NOT NULL DEFAULT FALSE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (plugin_id, ext_key)
);

ALTER TABLE content_fields ADD COLUMN owner TEXT NOT NULL DEFAULT 'admin';
