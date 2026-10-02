-- 0010_themes: versioned theme rows (tokens + layout + optional Tera
-- templates). Themes are data, never code; is_active pins one per site.

CREATE TABLE themes (
    id              BIGINT PRIMARY KEY,
    name            TEXT NOT NULL,
    version         INTEGER NOT NULL DEFAULT 1,
    is_active       BOOLEAN NOT NULL DEFAULT false,
    tokens          JSONB NOT NULL,
    layout          JSONB NOT NULL,
    templates       JSONB,
    parent_theme_id BIGINT REFERENCES themes (id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX themes_name_version_uniq ON themes (name, version);
CREATE INDEX themes_active_idx ON themes (is_active) WHERE is_active;

-- Only one active theme: partial unique index.
CREATE UNIQUE INDEX themes_single_active_uniq
    ON themes ((true)) WHERE is_active;
