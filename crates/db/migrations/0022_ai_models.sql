-- 0022_ai_models: registered AI providers and models.
--
-- ai_providers holds one row per provider (anthropic, openai, openrouter)
-- with its API key. The key is stored encrypted when the server has a
-- VYASA_SECRET_KEY, and plain otherwise — the prefix says which, and the
-- admin page says so too. Responses never include it.
--
-- ai_models is the registry the features draw from: each row is one model
-- id at one provider for one kind of job (text, vision, image, embedding,
-- moderation, transcription, speech). At most one model per kind is the
-- default; the partial unique index enforces that in the database rather
-- than in code that could forget.

CREATE TABLE ai_providers (
    provider   TEXT PRIMARY KEY,
    api_key    TEXT NOT NULL DEFAULT '',
    base_url   TEXT NOT NULL DEFAULT '',
    enabled    BOOLEAN NOT NULL DEFAULT true,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE ai_models (
    id                     BIGINT PRIMARY KEY,
    provider               TEXT NOT NULL REFERENCES ai_providers (provider) ON DELETE CASCADE,
    model                  TEXT NOT NULL,
    kind                   TEXT NOT NULL,
    label                  TEXT NOT NULL DEFAULT '',
    enabled                BOOLEAN NOT NULL DEFAULT true,
    is_default             BOOLEAN NOT NULL DEFAULT false,
    settings               JSONB NOT NULL DEFAULT '{}'::jsonb,
    input_cost_per_mtok    DOUBLE PRECISION,
    output_cost_per_mtok   DOUBLE PRECISION,
    last_probe_ok          BOOLEAN,
    last_probe_detail      TEXT NOT NULL DEFAULT '',
    last_probe_at          TIMESTAMPTZ,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, model, kind)
);

CREATE UNIQUE INDEX ai_models_default_per_kind ON ai_models (kind) WHERE is_default;
CREATE INDEX ai_models_kind_idx ON ai_models (kind, enabled);
