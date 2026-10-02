-- 0027_custom_taxonomies: let plugins register their own taxonomies.
--
-- Same change as 0026 made for post types, for the same reason: the old
-- constraint listed the two built-in taxonomies, so a plugin's taxonomy
-- was unstorable no matter what the application allowed. Replaced with a
-- name grammar rather than dropped — the column still refuses anything
-- that is not a plausible slug, since a taxonomy name ends up in a URL.
ALTER TABLE terms DROP CONSTRAINT terms_taxonomy_check;

ALTER TABLE terms ADD CONSTRAINT terms_taxonomy_check
    CHECK (taxonomy ~ '^[a-z][a-z0-9-]{1,31}$' AND taxonomy NOT LIKE '%-');
