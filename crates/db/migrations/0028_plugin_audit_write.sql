-- 0028_plugin_audit_write: let the audit log record writes.
--
-- `kind` was constrained to ('deny', 'fetch', 'quota') when the only
-- broker decisions were reads and fetches. Plugin writes then started
-- recording `kind = 'write'`, which the constraint refused — and because
-- the insert's result was discarded, every write audit since has been
-- silently dropped. The constraint stays (a typo'd kind should still be
-- rejected); it just knows about writes now.
ALTER TABLE plugin_audit DROP CONSTRAINT plugin_audit_kind_check;

ALTER TABLE plugin_audit ADD CONSTRAINT plugin_audit_kind_check
    CHECK (kind IN ('deny', 'fetch', 'quota', 'write'));
