-- 0044_patterns: saved arrangements of blocks. A synced pattern is
-- referenced by id from documents and resolved at render time, so editing
-- it changes every page that uses it; an unsynced one is copied on insert.

CREATE TABLE patterns (
    id         BIGINT PRIMARY KEY,
    name       TEXT NOT NULL,
    slug       TEXT NOT NULL UNIQUE,
    category   TEXT NOT NULL DEFAULT '',
    synced     BOOLEAN NOT NULL DEFAULT false,
    blocks     JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_by BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
