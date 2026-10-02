-- 0001_users: identity table. IDs are application-generated (snowflake,
-- phase 02 IdGen) and stored as BIGINT. citext gives case-insensitive
-- uniqueness for email/username (the classic WP login footgun).

CREATE EXTENSION IF NOT EXISTS citext;

CREATE TABLE users (
    id              BIGINT PRIMARY KEY,
    email           CITEXT NOT NULL UNIQUE,
    username        CITEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    password_hash   TEXT,
    role            TEXT NOT NULL
                    CONSTRAINT users_role_check
                    CHECK (role IN ('admin', 'editor', 'author', 'contributor', 'subscriber')),
    bio             TEXT NOT NULL DEFAULT '',
    avatar_media_id BIGINT,
    meta            JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX users_email_idx ON users (email);
CREATE INDEX users_username_idx ON users (username);

-- updated_at is touched by the repositories on every UPDATE; the trigger
-- keeps it honest even for ad-hoc SQL.
CREATE FUNCTION users_touch_updated_at() RETURNS trigger AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER users_updated_at BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION users_touch_updated_at();
