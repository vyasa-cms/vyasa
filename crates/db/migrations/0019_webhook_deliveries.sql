-- 0019_webhook_deliveries: delivery attempt log per webhook.

CREATE TABLE webhook_deliveries (
    id            BIGINT PRIMARY KEY,
    webhook_id    BIGINT NOT NULL REFERENCES webhooks (id) ON DELETE CASCADE,
    event         TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('success', 'failed', 'dead')),
    response_code INTEGER,
    attempts      INTEGER NOT NULL DEFAULT 1,
    last_attempt  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX webhook_deliveries_webhook_idx
    ON webhook_deliveries (webhook_id, last_attempt DESC);
