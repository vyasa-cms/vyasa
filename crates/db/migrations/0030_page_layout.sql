-- 0030_page_layout: let a page compose itself.
--
-- Until now a page was a title and a block document rendered through the
-- theme's single `page` template, so every page on a site had the same
-- shape. An author who wanted a landing page for one campaign — hero, three
-- features, a dark call-to-action band — had no way to say so.
--
-- The column holds the same section tree a theme layout holds, scoped to
-- one page. NULL means "render through the theme template", which is what
-- every existing row means and why no backfill is needed.
ALTER TABLE posts ADD COLUMN layout JSONB;

-- Revisioned with the content it composes: reverting a page to Tuesday has
-- to restore Tuesday's arrangement, not leave last night's sections around
-- the restored text.
ALTER TABLE post_revisions ADD COLUMN layout JSONB;
