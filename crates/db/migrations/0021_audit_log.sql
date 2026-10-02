-- 0021_audit_log: who did what, for the admin actions that are hard to undo.
--
-- Distinct from plugin_audit (0017), which records capability-broker
-- decisions made *by* plugins. This records decisions made by people.
--
-- actor_id is nullable and not a foreign key: the log has to survive the
-- deletion of the account that acted, which is exactly the case it exists
-- to explain.

CREATE TABLE audit_log (
    id         BIGINT PRIMARY KEY,
    actor_id   BIGINT,
    actor_name TEXT NOT NULL DEFAULT '',
    action     TEXT NOT NULL,
    target     TEXT NOT NULL DEFAULT '',
    detail     JSONB NOT NULL DEFAULT '{}'::jsonb,
    ip         TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX audit_log_created_idx ON audit_log (created_at DESC);
CREATE INDEX audit_log_actor_idx ON audit_log (actor_id, created_at DESC);
CREATE INDEX audit_log_action_idx ON audit_log (action, created_at DESC);
