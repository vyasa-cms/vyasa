-- 0029_theme_assets: let a theme ship its own CSS and JavaScript.
--
-- Themes could set design tokens and compose layouts but not style
-- anything the tokens did not already describe, and could not add a line
-- of behaviour. That was the largest practical limit on how far a theme
-- could go — a plugin could ship assets and a theme could not.
--
-- Stored on the row rather than as files: a theme is already a database
-- record (tokens, layout, template overrides), the assets version with it,
-- and rolling back a theme rolls back its stylesheet with everything else.
ALTER TABLE themes      ADD COLUMN assets JSONB;
ALTER TABLE theme_drafts ADD COLUMN assets JSONB;

-- Revisions carry them too, so studio history can restore them.
ALTER TABLE theme_draft_revisions ADD COLUMN assets JSONB;
