-- 0016_import_keys: idempotency pointers for the WordPress importer.

ALTER TABLE comments ADD COLUMN import_key TEXT;
CREATE INDEX comments_import_key_idx ON comments (import_key) WHERE import_key IS NOT NULL;
CREATE INDEX users_meta_import_idx ON users USING gin (meta);
