-- 0012_jobs: Postgres-backed job queue. Claiming uses FOR UPDATE SKIP
-- LOCKED so future multi-worker deployments are safe.

CREATE TABLE jobs (
    id         BIGINT PRIMARY KEY,
    kind       TEXT NOT NULL,
    payload    JSONB NOT NULL DEFAULT '{}'::jsonb,
    run_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    status     TEXT NOT NULL DEFAULT 'queued'
               CONSTRAINT jobs_status_check
               CHECK (status IN ('queued', 'running', 'done', 'failed', 'dead')),
    attempts   INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX jobs_claim_idx ON jobs (status, run_at)
    WHERE status = 'queued';
CREATE INDEX jobs_dead_idx ON jobs (created_at DESC) WHERE status = 'dead';
