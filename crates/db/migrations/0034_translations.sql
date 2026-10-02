-- Translations (phase 62): an entry can declare its language and share a
-- group with its translations. Kept out of `posts` on purpose — the
-- posts row and its many SELECT lists stay untouched, and an entry
-- without a row simply has no language story.
CREATE TABLE post_translations (
    post_id  BIGINT PRIMARY KEY REFERENCES posts (id) ON DELETE CASCADE,
    lang     TEXT   NOT NULL,
    group_id BIGINT NOT NULL
);
CREATE INDEX post_translations_group_idx ON post_translations (group_id);
