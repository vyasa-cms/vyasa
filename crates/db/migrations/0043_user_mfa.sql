-- 0043_user_mfa: a second factor per account. The TOTP secret is sealed
-- with the server secret; recovery codes are stored hashed and each one
-- is spent on use.

CREATE TABLE user_mfa (
    user_id        BIGINT PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    secret         TEXT NOT NULL,
    enabled_at     TIMESTAMPTZ,
    recovery_codes JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
