-- 0023_ai_features: storage for the AI integrations.
--
-- post_embeddings: one vector per post from the registered embedding model,
-- keyed by a hash of the text it was computed from so an unchanged post is
-- never re-embedded. A REAL[] is enough at blog scale; pgvector can replace
-- it without changing callers.
--
-- comments.moderation: the screening verdict, kept next to the comment so
-- the queue can show why something was flagged.
--
-- media.transcript: text for audio/video files.
--
-- post_audio: the read-aloud recording for a post, as a media row, keyed by
-- the same text hash so a re-publish with no text change keeps the audio.

CREATE TABLE post_embeddings (
    post_id    BIGINT PRIMARY KEY REFERENCES posts (id) ON DELETE CASCADE,
    model      TEXT NOT NULL,
    text_hash  TEXT NOT NULL,
    dims       INTEGER NOT NULL,
    vector     REAL[] NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE comments ADD COLUMN moderation JSONB;

ALTER TABLE media ADD COLUMN transcript TEXT;

CREATE TABLE post_audio (
    post_id    BIGINT PRIMARY KEY REFERENCES posts (id) ON DELETE CASCADE,
    media_id   BIGINT NOT NULL REFERENCES media (id) ON DELETE CASCADE,
    model      TEXT NOT NULL,
    text_hash  TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
