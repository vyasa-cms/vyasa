-- 0008_comments: threaded discussion with moderation statuses.

CREATE TABLE comments (
    id             BIGINT PRIMARY KEY,
    post_id        BIGINT NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    author_user_id BIGINT REFERENCES users (id) ON DELETE SET NULL,
    author_name    TEXT NOT NULL,
    author_email   TEXT NOT NULL,
    content        TEXT NOT NULL,
    parent_id      BIGINT REFERENCES comments (id) ON DELETE CASCADE,
    status         TEXT NOT NULL DEFAULT 'pending'
                   CONSTRAINT comments_status_check
                   CHECK (status IN ('pending', 'approved', 'spam', 'trash')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX comments_post_status_idx ON comments (post_id, status);
CREATE INDEX comments_status_created_idx ON comments (status, created_at DESC);
CREATE INDEX comments_parent_idx ON comments (parent_id);
