-- Audience (phase 61): cookieless view rollups, lead-form submissions,
-- newsletter subscribers.

-- One row per day × path × referrer host. No IPs, no cookies, no user
-- agents stored: the server counts its own HTML responses and keeps
-- only the aggregate — the whole point is that there is nothing here a
-- privacy policy has to apologise for.
CREATE TABLE page_views (
    day       DATE   NOT NULL,
    path      TEXT   NOT NULL,
    referrer  TEXT   NOT NULL DEFAULT '',
    views     BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (day, path, referrer)
);
CREATE INDEX page_views_day_idx ON page_views (day);

-- What a visitor typed into a page's signup/contact form.
CREATE TABLE form_submissions (
    id         BIGINT      PRIMARY KEY,
    form       TEXT        NOT NULL,
    name       TEXT        NOT NULL DEFAULT '',
    email      TEXT        NOT NULL,
    message    TEXT        NOT NULL DEFAULT '',
    path       TEXT        NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX form_submissions_created_idx ON form_submissions (created_at DESC);

-- Newsletter subscribers. Double opt-in: rows are born pending and only
-- a click on the emailed token confirms; the same token later
-- unsubscribes, so every email the system sends carries a working exit.
CREATE TABLE subscribers (
    id           BIGINT      PRIMARY KEY,
    email        TEXT        NOT NULL UNIQUE,
    status       TEXT        NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'confirmed', 'unsubscribed')),
    token        TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    confirmed_at TIMESTAMPTZ
);
CREATE INDEX subscribers_status_idx ON subscribers (status);
