-- 0003_api_keys: scoped headless API tokens. Only the key hash is stored;
-- the raw key is shown once at creation (phase 08).

CREATE TABLE api_keys (
    id           BIGINT PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    key_hash     TEXT NOT NULL UNIQUE,
    capabilities JSONB NOT NULL DEFAULT '[]'::jsonb,
    last_used_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ
);

CREATE INDEX api_keys_user_id_idx ON api_keys (user_id);
CREATE INDEX api_keys_key_hash_idx ON api_keys (key_hash);
