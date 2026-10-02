-- 0013_webhooks_ai: outbound webhooks and AI usage/cost accounting.

CREATE TABLE webhooks (
    id         BIGINT PRIMARY KEY,
    url        TEXT NOT NULL,
    secret     TEXT NOT NULL,
    events     JSONB NOT NULL DEFAULT '[]'::jsonb,
    enabled    BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX webhooks_enabled_idx ON webhooks (enabled) WHERE enabled;

CREATE TABLE ai_logs (
    id                BIGINT PRIMARY KEY,
    provider          TEXT NOT NULL,
    model             TEXT NOT NULL,
    purpose           TEXT NOT NULL,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd          DOUBLE PRECISION NOT NULL DEFAULT 0,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ai_logs_created_idx ON ai_logs (created_at DESC);
CREATE INDEX ai_logs_purpose_idx ON ai_logs (purpose, created_at DESC);
