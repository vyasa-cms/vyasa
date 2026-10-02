-- 0045_sticky_and_media_trash: a post pinned to the top of the home
-- listing, and a trash for media so a deleted file can come back.

ALTER TABLE posts ADD COLUMN sticky BOOLEAN NOT NULL DEFAULT false;
CREATE INDEX posts_sticky_idx ON posts (sticky) WHERE sticky;

ALTER TABLE media ADD COLUMN trashed_at TIMESTAMPTZ;
CREATE INDEX media_trashed_idx ON media (trashed_at) WHERE trashed_at IS NOT NULL;
