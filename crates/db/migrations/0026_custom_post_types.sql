-- 0026_custom_post_types: let plugins register their own post types.
--
-- The old constraint listed the three built-in kinds, so a custom type was
-- unstorable no matter what the application allowed. It is replaced with a
-- name grammar rather than dropped: the column still refuses anything that
-- is not a plausible slug, so a bug upstream cannot write `../etc` into a
-- field that ends up in a URL.
ALTER TABLE posts DROP CONSTRAINT posts_type_check;

ALTER TABLE posts ADD CONSTRAINT posts_type_check
    CHECK (type ~ '^[a-z][a-z0-9-]{1,31}$' AND type NOT LIKE '%-');
