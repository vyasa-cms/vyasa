-- 0050_registration: confirmed email addresses and purpose-bound tokens
-- (phase 98, public registration).
--
-- `users.email_verified_at` is NULL only for an account somebody made for
-- themselves through registration and has not confirmed yet. Every account
-- that exists today was made by an administrator or the setup wizard, so
-- it is confirmed as of its creation.
--
-- The column is added without a default (a metadata-only change), filled,
-- and only then given `DEFAULT now()`: every way of creating an account
-- other than registration -- an administrator, an invitation, the setup
-- wizard, the CLI, an import, and a binary from before this migration
-- during a rolling deploy -- keeps producing confirmed accounts without
-- knowing the column exists. Registration writes NULL explicitly.

ALTER TABLE users ADD COLUMN email_verified_at TIMESTAMPTZ;

-- The backfill is bookkeeping, not an edit to anyone's account: keep it
-- from stamping `updated_at` on every row.
ALTER TABLE users DISABLE TRIGGER users_updated_at;
UPDATE users SET email_verified_at = created_at;
ALTER TABLE users ENABLE TRIGGER users_updated_at;

ALTER TABLE users ALTER COLUMN email_verified_at SET DEFAULT now();

-- What the purge of stale unconfirmed accounts scans.
CREATE INDEX users_unconfirmed_idx ON users (created_at) WHERE email_verified_at IS NULL;

-- One token store, two purposes. A token is only ever accepted for the
-- purpose it was issued for. Existing rows are password-reset (and
-- invitation) tokens; a constant default does not rewrite the table.
ALTER TABLE reset_tokens
    ADD COLUMN purpose TEXT NOT NULL DEFAULT 'reset'
        CONSTRAINT reset_tokens_purpose_check CHECK (purpose IN ('reset', 'verify'));
