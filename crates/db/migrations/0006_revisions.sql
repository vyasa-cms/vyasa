-- 0006_revisions: post snapshots. Pruning policy (dense recent, sparse
-- old) is applied by the RevisionService (phase 12), not the DB.

CREATE TABLE post_revisions (
    id         BIGINT PRIMARY KEY,
    post_id    BIGINT NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    title      TEXT NOT NULL,
    content    JSONB NOT NULL,
    author_id  BIGINT NOT NULL REFERENCES users (id),
    is_autosave BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX post_revisions_post_idx ON post_revisions (post_id, created_at DESC);
