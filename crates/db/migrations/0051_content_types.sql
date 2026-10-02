-- 0051_content_types: content types and custom fields managed from the
-- admin (phase 99).
--
-- `content_types` holds the types an administrator created; each slug is
-- a `posts.type` value and a URL segment, so its grammar is the posts
-- column's own (0026) — the application narrows it further (reserved
-- words, plugin-declared types). Built-in and plugin types are not rows
-- here.
--
-- `content_fields` defines fields on any type: the built-in `post` and
-- `page`, an administrator's type, or a plugin's. There is no foreign key
-- to `content_types` for that reason; deleting an administrator's type
-- deletes its fields in the same transaction (application side).
--
-- Field values live in `posts.meta.fields`; revisions and autosaves keep
-- their own copy in the new `post_revisions.fields` column. NULL there
-- means the revision predates fields (restoring it keeps the entry's
-- current values), not "no values".
--
-- Safe on a live database: two new tables, and a nullable column with no
-- default on `post_revisions` (a metadata-only change, no rewrite).

CREATE TABLE content_types (
    slug        TEXT PRIMARY KEY
                CHECK (slug ~ '^[a-z][a-z0-9-]{1,31}$' AND slug NOT LIKE '%-'),
    singular    TEXT NOT NULL CHECK (char_length(singular) BETWEEN 1 AND 80),
    plural      TEXT NOT NULL CHECK (char_length(plural) BETWEEN 1 AND 80),
    description TEXT NOT NULL DEFAULT '' CHECK (char_length(description) <= 500),
    public      BOOLEAN NOT NULL DEFAULT TRUE,
    has_archive BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE content_fields (
    type_slug  TEXT NOT NULL
               CHECK (type_slug ~ '^[a-z][a-z0-9-]{1,31}$' AND type_slug NOT LIKE '%-'),
    key        TEXT NOT NULL CHECK (key ~ '^[a-z][a-z0-9_]{0,39}$'),
    label      TEXT NOT NULL CHECK (char_length(label) BETWEEN 1 AND 120),
    help       TEXT NOT NULL DEFAULT '' CHECK (char_length(help) <= 500),
    kind       TEXT NOT NULL
               CHECK (kind IN ('text', 'textarea', 'number', 'boolean', 'date',
                               'choice', 'url', 'media', 'entry')),
    required   BOOLEAN NOT NULL DEFAULT FALSE,
    options    JSONB NOT NULL DEFAULT '{}'::jsonb
               CHECK (jsonb_typeof(options) = 'object'),
    position   INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (type_slug, key)
);

-- The ALTER needs an ACCESS EXCLUSIVE lock on `post_revisions` for an
-- instant, but waiting for it would queue every revision read and write
-- behind a long-running transaction. Give up after five seconds instead
-- (the migration fails cleanly and can be retried at a quieter moment).
-- sqlx runs each migration in a transaction, so `SET LOCAL` ends with it.
SET LOCAL lock_timeout = '5s';
ALTER TABLE post_revisions ADD COLUMN fields JSONB;
