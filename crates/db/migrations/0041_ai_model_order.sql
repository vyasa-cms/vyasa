-- 0041_ai_model_order: the fallback order within a kind, set from the
-- admin page. The default still comes first; this orders the rest.

ALTER TABLE ai_models ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
