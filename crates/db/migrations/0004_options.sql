-- 0004_options: typed JSONB site settings, keyed by name. Values are
-- validated by the OptionsService (phase 27); the DB stores shape-agnostic
-- JSON only.

CREATE TABLE options (
    key        TEXT PRIMARY KEY,
    value      JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
