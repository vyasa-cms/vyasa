-- 0031_assistant_proposals: the assistant proposes, the author accepts.
--
-- It used to write a revision the moment it answered. Undo made that
-- survivable, but a change you did not ask for still landed on the draft
-- and had to be walked back — and there was no way to look at what it
-- wanted to do before it did it.
--
-- A reply now carries the documents it would write. Nothing is committed
-- until someone accepts, so the preview can render a proposal without the
-- draft moving.
ALTER TABLE theme_draft_messages ADD COLUMN proposal JSONB;

-- The revision it was composed against. Accepting a proposal built on an
-- older draft would silently discard whatever was done in between, so the
-- accept checks this and refuses rather than clobbering.
ALTER TABLE theme_draft_messages ADD COLUMN proposal_base INTEGER;
