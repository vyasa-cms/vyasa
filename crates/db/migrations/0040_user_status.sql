-- 0040_user_status: when an account last signed in, and whether an
-- administrator has suspended it.

ALTER TABLE users
    ADD COLUMN last_login_at TIMESTAMPTZ,
    ADD COLUMN suspended_at  TIMESTAMPTZ;
