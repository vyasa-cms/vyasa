-- 0057_audience_analytics: cookieless audience analytics beyond the per-path view counter.
--
-- Aggregates only. `audience_counts` holds one counter per day, dimension and value (bot hits, device and browser
-- classes, referrer campaign tags, entry pages, 404s with their referrers, search terms, outbound clicks,
-- conversions, unique visitors). `audience_visitors` holds the day's visitor hashes so a visitor counts once;
-- a hash is keyed by a secret that exists only in memory for one UTC day, and rows of earlier days are deleted as
-- the next day's first visit arrives, so no identifier outlives its day. No IP address and no user agent is stored.

CREATE TABLE audience_counts (
    day   DATE   NOT NULL,
    dim   TEXT   NOT NULL,
    key   TEXT   NOT NULL,
    extra TEXT   NOT NULL DEFAULT '',
    n     BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (day, dim, key, extra)
);
CREATE INDEX audience_counts_dim_day_idx ON audience_counts (dim, day);

CREATE TABLE audience_visitors (
    day  DATE  NOT NULL,
    hash BYTEA NOT NULL,
    PRIMARY KEY (day, hash)
);

-- `/go/<name>`: a server-side redirect table, so an outbound click is counted without a script. A link marked
-- `conversion` also counts as a conversion (a "copy install" link).
CREATE TABLE audience_links (
    name       TEXT        PRIMARY KEY,
    url        TEXT        NOT NULL,
    conversion BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
