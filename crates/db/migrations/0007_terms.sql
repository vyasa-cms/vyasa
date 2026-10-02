-- 0007_terms: normalized taxonomy (categories/tags/custom) and their
-- post relationships. Replace-set semantics on assignment (phase 13).

CREATE TABLE terms (
    id        BIGINT PRIMARY KEY,
    taxonomy  TEXT NOT NULL
              CONSTRAINT terms_taxonomy_check
              CHECK (taxonomy IN ('category', 'tag')),
    name      TEXT NOT NULL,
    slug      TEXT NOT NULL,
    parent_id BIGINT REFERENCES terms (id) ON DELETE SET NULL,
    meta      JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX terms_taxonomy_slug_uniq ON terms (taxonomy, slug);
CREATE INDEX terms_parent_idx ON terms (parent_id);

CREATE TABLE term_relationships (
    post_id BIGINT NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    term_id BIGINT NOT NULL REFERENCES terms (id) ON DELETE CASCADE,
    PRIMARY KEY (post_id, term_id)
);

CREATE INDEX term_relationships_term_idx ON term_relationships (term_id);
