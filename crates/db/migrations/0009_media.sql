-- 0009_media: media library metadata. Bytes live on the filesystem (or
-- S3 later); derived sizes and blurhash land in `derivatives` (phase 16).

CREATE TABLE media (
    id          BIGINT PRIMARY KEY,
    owner_id    BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    file_name   TEXT NOT NULL,
    mime        TEXT NOT NULL,
    byte_size   BIGINT NOT NULL,
    storage     TEXT NOT NULL DEFAULT 'local'
                CONSTRAINT media_storage_check
                CHECK (storage IN ('local', 's3')),
    path        TEXT NOT NULL,
    width       INTEGER,
    height      INTEGER,
    blurhash    TEXT,
    alt         TEXT,
    caption     TEXT,
    derivatives JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX media_owner_idx ON media (owner_id);
CREATE INDEX media_created_idx ON media (created_at DESC);
