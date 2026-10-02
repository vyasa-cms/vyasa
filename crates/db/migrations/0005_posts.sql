-- 0005_posts: content rows. Content is a structured block array (JSONB);
-- rendered HTML is a cache, never the source of truth. Slug uniqueness is
-- partial (excludes trash) so trashed slugs can be reused; restore must
-- re-check (PostService, phase 11).

CREATE TABLE posts (
    id            BIGINT PRIMARY KEY,
    type          TEXT NOT NULL DEFAULT 'post'
                  CONSTRAINT posts_type_check
                  CHECK (type IN ('post', 'page', 'block')),
    status        TEXT NOT NULL DEFAULT 'draft'
                  CONSTRAINT posts_status_check
                  CHECK (status IN ('draft', 'scheduled', 'published', 'private', 'trash')),
    slug          TEXT NOT NULL,
    title         TEXT NOT NULL,
    content       JSONB NOT NULL DEFAULT '[]'::jsonb,
    excerpt       TEXT,
    author_id     BIGINT NOT NULL REFERENCES users (id),
    parent_id     BIGINT REFERENCES posts (id) ON DELETE SET NULL,
    meta          JSONB NOT NULL DEFAULT '{}'::jsonb,
    published_at  TIMESTAMPTZ,
    scheduled_for TIMESTAMPTZ,
    password_hash TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX posts_type_slug_uniq
    ON posts (type, slug) WHERE status <> 'trash';
CREATE INDEX posts_status_published_idx
    ON posts (status, published_at DESC) WHERE status = 'published';
CREATE INDEX posts_author_idx ON posts (author_id);
CREATE INDEX posts_type_idx ON posts (type);
CREATE INDEX posts_scheduled_idx
    ON posts (scheduled_for) WHERE status = 'scheduled';

CREATE FUNCTION posts_touch_updated_at() RETURNS trigger AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER posts_updated_at BEFORE UPDATE ON posts
    FOR EACH ROW EXECUTE FUNCTION posts_touch_updated_at();
