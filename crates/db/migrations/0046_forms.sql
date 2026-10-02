-- 0046_forms: forms an editor designs, and the answers they collect.
-- The fixed contact form keeps working; a defined form adds its own
-- fields, kept as JSON on the submission.

CREATE TABLE forms (
    id              BIGINT PRIMARY KEY,
    name            TEXT NOT NULL,
    slug            TEXT NOT NULL UNIQUE,
    fields          JSONB NOT NULL DEFAULT '[]'::jsonb,
    notify_email    TEXT NOT NULL DEFAULT '',
    success_message TEXT NOT NULL DEFAULT 'Thanks — your message has been received.',
    enabled         BOOLEAN NOT NULL DEFAULT true,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE form_submissions
    ADD COLUMN data    JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN read_at TIMESTAMPTZ;
CREATE INDEX form_submissions_form_idx ON form_submissions (form, created_at DESC);
