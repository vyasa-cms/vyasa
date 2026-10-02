-- 0024_theme_drafts: the theme studio's working copies.
--
-- A draft holds the three documents of a theme while someone (or the
-- assistant) works on it. Nothing here is ever rendered to visitors: a
-- draft becomes a `themes` row only when it is published. Every change is
-- kept as a revision so the studio can undo and compare; the conversation
-- with the assistant is kept so it survives a reload.

CREATE TABLE theme_drafts (
    id             BIGINT PRIMARY KEY,
    name           TEXT NOT NULL,
    base_theme_id  BIGINT REFERENCES themes (id) ON DELETE SET NULL,
    -- ready | generating | failed
    status         TEXT NOT NULL DEFAULT 'ready',
    status_note    TEXT,
    tokens         JSONB NOT NULL,
    layout         JSONB NOT NULL,
    templates      JSONB,
    -- Sequence number of the revision the documents above correspond to.
    revision       INTEGER NOT NULL DEFAULT 1,
    created_by     BIGINT REFERENCES users (id) ON DELETE SET NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX theme_drafts_updated_idx ON theme_drafts (updated_at DESC);

CREATE TABLE theme_draft_revisions (
    id          BIGINT PRIMARY KEY,
    draft_id    BIGINT NOT NULL REFERENCES theme_drafts (id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    tokens      JSONB NOT NULL,
    layout      JSONB NOT NULL,
    templates   JSONB,
    -- What changed, in words ("Changed colors.primary.light").
    note        TEXT NOT NULL,
    -- you | assistant | start | revert
    source      TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (draft_id, seq)
);

CREATE TABLE theme_draft_messages (
    id          BIGINT PRIMARY KEY,
    draft_id    BIGINT NOT NULL REFERENCES theme_drafts (id) ON DELETE CASCADE,
    -- you | assistant
    role        TEXT NOT NULL,
    text        TEXT NOT NULL,
    -- Revision the assistant produced in reply, when it changed something.
    revision    INTEGER,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX theme_draft_messages_draft_idx ON theme_draft_messages (draft_id, created_at);
