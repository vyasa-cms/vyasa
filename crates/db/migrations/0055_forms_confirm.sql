-- 0055_forms_confirm: a defined form can require the submitter to confirm
-- by email. A row with a token and no confirmed_at is pending: the inbox,
-- the CSV and the unread counts leave it out, and it expires.

ALTER TABLE forms
    ADD COLUMN confirm BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE form_submissions
    ADD COLUMN token        TEXT,
    ADD COLUMN confirmed_at TIMESTAMPTZ;
CREATE UNIQUE INDEX form_submissions_token_idx ON form_submissions (token) WHERE token IS NOT NULL;
