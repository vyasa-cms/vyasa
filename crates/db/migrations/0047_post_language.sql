-- 0047_post_language: which language an entry is written in, and which
-- entries are translations of one another (they share a group).

ALTER TABLE posts
    ADD COLUMN lang              TEXT NOT NULL DEFAULT '',
    ADD COLUMN translation_group BIGINT;
CREATE INDEX posts_translation_group_idx ON posts (translation_group) WHERE translation_group IS NOT NULL;
