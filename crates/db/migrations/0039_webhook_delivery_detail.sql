-- 0039_webhook_delivery_detail: what was sent and what came back, so a
-- failing receiver can be debugged from the admin panel and a dead
-- delivery can be sent again.

ALTER TABLE webhook_deliveries
    ADD COLUMN payload       TEXT,
    ADD COLUMN response_body TEXT,
    ADD COLUMN error         TEXT;
