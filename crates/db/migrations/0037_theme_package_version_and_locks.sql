-- Theme versions (phase 74): `version` is the local sequence every
-- install and studio publish advances; `package_version` is what the
-- package's manifest said. They used to be one column, so a theme edited
-- in the studio reached numbers the marketplace would later publish and
-- could never take the update.
ALTER TABLE themes ADD COLUMN package_version INTEGER;
UPDATE themes SET package_version = version WHERE parent_theme_id IS NULL;

-- Who has an entry open in the editor, so two people do not overwrite
-- each other without knowing. A row older than the heartbeat window is
-- stale and is taken over silently.
CREATE TABLE post_locks (
    post_id      BIGINT PRIMARY KEY REFERENCES posts (id) ON DELETE CASCADE,
    user_id      BIGINT NOT NULL,
    display_name TEXT NOT NULL,
    seen_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
