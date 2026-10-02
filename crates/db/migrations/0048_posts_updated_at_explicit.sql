-- 0048_posts_updated_at_explicit: `updated_at` means "last edited".
--
-- The 0005 trigger stamped now() on every UPDATE, so pinning a post,
-- setting its password or language, a status flip by the publisher, an
-- ownership transfer -- anything at all -- read as an edit, and the
-- service's own decision about whether a save touched the entry was
-- overwritten. The trigger now only fills in the stamp when the statement
-- left it alone AND an edited column (title, content, excerpt, slug,
-- layout) actually changed. A statement that sets updated_at itself is
-- respected, whatever value it writes.
--
-- Replacing the function body is a catalog update: no table rewrite, no
-- lock beyond the moment of the swap, safe on a live database.

CREATE OR REPLACE FUNCTION posts_touch_updated_at() RETURNS trigger AS $$
BEGIN
    IF NEW.updated_at IS NOT DISTINCT FROM OLD.updated_at
       AND (NEW.title, NEW.content, NEW.excerpt, NEW.slug, NEW.layout)
           IS DISTINCT FROM (OLD.title, OLD.content, OLD.excerpt, OLD.slug, OLD.layout)
    THEN
        NEW.updated_at = now();
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
