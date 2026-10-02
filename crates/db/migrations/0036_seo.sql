-- SEO support (phase 73).
--
-- Redirects: a published entry whose address changes keeps its old one
-- answering with a 301, written by the server at the moment of the
-- change. Path-keyed, site-relative.
CREATE TABLE redirects (
    from_path  TEXT PRIMARY KEY,
    to_path    TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Link checks: the last sweep of an entry's outbound links, so the editor
-- and the dashboard can show what is broken without fetching again.
CREATE TABLE link_checks (
    post_id    BIGINT PRIMARY KEY REFERENCES posts (id) ON DELETE CASCADE,
    checked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    broken     JSONB NOT NULL DEFAULT '[]'::jsonb
);
