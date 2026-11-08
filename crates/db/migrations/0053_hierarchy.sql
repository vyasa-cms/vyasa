-- 0053_hierarchy: order and nesting for post types that declare it (0.3).
--
-- `posts.parent_id` has existed since 0005. What was missing is an order
-- among siblings (`position`) and an address that does not depend on a
-- slug being unique across the whole type (`path`).
--
-- `path` is the slash-joined slugs from the root to the entry
-- (`guides/install`), kept by the application, and is NULL for every entry
-- of a type that is not hierarchical -- which is every existing row. The
-- slug index is therefore narrowed to rows without a path, so it guards
-- exactly what it guarded before; a hierarchical type is guarded by
-- `(type, path)` instead, which lets cousins share a slug.
--
-- The new slug index is built under a temporary name and renamed, so
-- uniqueness is never unenforced, and keeps the name `posts_type_slug_uniq`
-- that the slug probes refer to.
--
-- Safe on a live database: the two ADD COLUMNs are metadata-only (a constant
-- default and a nullable column), and the index builds scan `posts` once.
-- sqlx runs each migration in a transaction, so `SET LOCAL` ends with it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE posts ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
ALTER TABLE posts ADD COLUMN path TEXT;
ALTER TABLE content_types ADD COLUMN hierarchical BOOLEAN NOT NULL DEFAULT FALSE;

CREATE UNIQUE INDEX posts_type_slug_uniq_v2
    ON posts (type, slug) WHERE status <> 'trash' AND path IS NULL;
DROP INDEX posts_type_slug_uniq;
ALTER INDEX posts_type_slug_uniq_v2 RENAME TO posts_type_slug_uniq;

CREATE UNIQUE INDEX posts_type_path_uniq
    ON posts (type, path) WHERE status <> 'trash' AND path IS NOT NULL;
CREATE INDEX posts_parent_position_idx
    ON posts (type, parent_id, position, id) WHERE status <> 'trash';
