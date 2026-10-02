# AI models

The admin's **AI models** page (`/admin/models`) is where a site registers
the providers it pays and the models its features may call. Nothing in the
product picks a model on its own: each kind of job has a registered
default, and a feature asks the registry for it.

## Providers

| Provider | API | Kinds it can serve |
|---|---|---|
| Anthropic | Messages API | text, vision |
| OpenAI | Platform API | text, vision, image, embedding, moderation, transcription, speech |
| OpenRouter | OpenAI-compatible gateway | text, vision, image, embedding |

Each provider has one API key and an optional base URL override (for a
proxy, or an OpenAI-compatible server). Keys are write-only: the API and
the page show only whether a key exists, where it comes from, and its last
four characters.

**Where a key comes from**, in order: the key saved on the page; else the
environment (`VYASA_AI_ANTHROPIC_KEY`, `VYASA_AI_OPENAI_KEY`,
`VYASA_AI_OPENROUTER_KEY`). The environment fallback keeps a deployment
that predates the page working.

**At rest**, a saved key is sealed with ChaCha20-Poly1305 under a key
derived from `VYASA_SECRET_KEY` when that is set, and stored plain when it
is not; the page says which. Set the secret before saving keys in
production. Changing the secret makes previously sealed keys unreadable —
the page reports this and asks for the key again.

## Kinds of job

| Kind | Used by |
|---|---|
| `text` | Writing assistant (`/ai/assist/*`), excerpt/SEO suggestions and autofill, theme builder, `/ai/generate`, plugins (`ai-complete`) |
| `vision` | Alt text for uploaded images |
| `image` | Image generation from the editor |
| `embedding` | Semantic search, the `related-posts` block |
| `moderation` | Comment screening |
| `transcription` | Transcripts for audio and video files |
| `speech` | Read-aloud recordings (the `read-aloud` block) |

A model is registered for exactly one kind; the same provider model can be
registered under several kinds (a multimodal model as both `text` and
`vision`). The first model registered for a kind becomes its default;
**Make default** moves it. The database enforces at most one default per
kind.

## How features call a model

`crates/api/src/ai_registry.rs` turns the registry into a failover chain
for a kind: the default first, then every other enabled model of that kind
in registration order, each behind its own circuit breaker (three
infrastructure failures open a slot for a minute; 4xx responses do not
count). The chain implements `LlmProvider`, so the structured runner and
the theme generator use it unchanged. The completion records which
provider and model actually answered, and the usage log (`ai_logs`) shows
that rather than the first choice.

With no text model registered, the chain falls back to the environment's
Anthropic key and `VYASA_AI_MODEL` (default `claude-haiku-4-5`).

## Testing a model

**Test** runs the cheapest real call that proves the model works:

- `text` — a one-line structured completion (a few dozen tokens, logged
  under the `model-probe` purpose);
- `embedding` — one short embedding, reporting the dimensionality;
- every other kind — a lookup in the provider's catalogue (generating an
  image to prove an image model exists would cost real money).

**Browse** in the add dialog lists what the key can actually use, filtered
to the chosen kind: from the provider's own metadata for OpenRouter and
Anthropic, and by naming convention for OpenAI, whose catalogue carries no
modality information.

## API

All endpoints require `ManageOptions`.

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/v1/ai/models` | providers, models and kinds in one response |
| POST | `/api/v1/ai/models` | register a model |
| PUT | `/api/v1/ai/models/{id}` | edit label, enabled, settings, costs |
| DELETE | `/api/v1/ai/models/{id}` | remove (the default passes to another enabled model) |
| POST | `/api/v1/ai/models/{id}/default` | make default for its kind |
| POST | `/api/v1/ai/models/{id}/test` | probe |
| PUT | `/api/v1/ai/providers/{provider}` | save key / base URL / enabled |
| DELETE | `/api/v1/ai/providers/{provider}` | remove the provider and its models |
| GET | `/api/v1/ai/providers/{provider}/catalog?q=&kind=` | what the key can use |
| POST | `/api/v1/ai/providers/{provider}/test` | can the key list models? |

Provider and model changes are written to the audit log
(`ai.provider.update`, `ai.provider.delete`, `ai.model.register`,
`ai.model.default`, `ai.model.delete`).

## Costs

Per-model input/output costs (USD per million tokens) are optional and
informational on the page. Usage accounting in `ai_logs` still prices with
the built-in table in `vyasa_ai::default_cost_table`; connecting the two
is the natural next step once more than one model is in use.

## Integrations

Every integration is **off until an operator turns it on** in Settings →
AI features, and each uses the default model of its kind. Work that a
person is not waiting for runs as a queue job (`ai_*` kinds, handled in
`crates/api/src/ai_jobs.rs`); the on-demand endpoints honour the same
switches. Storage is migration 0023.

| Feature | Switch | Trigger | Where it lands |
|---|---|---|---|
| Alt text | `ai_alt_text` | image upload without alt; **Generate** in the media library | `media.alt` |
| Comment screening | `ai_comment_screening` = `flag` / `spam` | `comment.added`; **Screen** on a comment | `comments.moderation` (shown in the queue); `spam` mode moves flagged comments |
| Autofill | `ai_autofill` | publish; `POST /posts/{id}/autofill` | empty `excerpt`, `meta.seo_description` (+ `seo_title`) |
| Suggest | — (text model) | **Suggest** buttons in the editor sidebar | excerpt, search preview fields |
| Embeddings | `ai_embeddings` | publish / update / trash; `POST /posts/{id}/embed` | `post_embeddings` (one vector per post, keyed by a text hash) |
| Semantic search | `ai_semantic_search` | every search | full-text candidates re-ordered by cosine similarity to the query |
| Related posts | `ai_related_posts` | theme block `related-posts`; `GET /posts/{id}/related` | nearest published posts |
| Image generation | `ai_images` | **Generate** in the editor's image block; `POST /ai/images` | a media item, capped at 50 a day site-wide |
| Transcription | `ai_transcription` | **Transcribe** in the media library and on audio/video blocks | `media.transcript`; insertable as a Details block |
| Read aloud | `ai_read_aloud` | publish; **Generate audio** in the sidebar | an MP3 media item + `post_audio`, rendered by the `read-aloud` block |
| Theme studio assistant | Text (Vision for images) | Appearance → studio → Assistant | Edits a theme draft through validated operations; never writes CSS |

Costs: usage rows are priced from the registry's per-model costs when set
(falling back to the built-in table); images, transcription and speech log
one row per call with the model's input rate as the cost, since those
APIs report no tokens.


## The agent harness (phase 57)

`vyasa_ai::agent` is the loop every agentic feature runs on: the model
returns one action per step — `{thought, tool, input, reply}`, with
`tool: "done"` to finish — acts through a **toolbox**, reads the
observation, and iterates. The protocol is a schema over the existing
structured-output path, so it works with every model the registry can
hold, free fallbacks included, and every step is an `ai_logs` row under
the run's purpose (the month cap sees agent work like any other spend).

**Adding a surface** means writing a toolbox, nothing else:

1. Implement `agent::Toolbox` — `tools()` returns the defs (name,
   description, input schema; they are rendered into the system prompt),
   `call()` runs one. Keep the surface's state *inside* the toolbox as a
   sandbox; return validator diagnostics as `Err(String)` observations so
   the model repairs its own mistakes.
2. If the agent should see rendered output, split the server-dependent
   half behind an "eyes" trait (see `theme_studio::StudioEyes` /
   `page_designer::PageEyes` and `crates/api/src/agent_eyes.rs`, which
   digests a render into section order + visible text).
3. Call `agent::run_agent` with the surface's own system prompt; turn
   the sandbox's end state into the surface's normal proposal.

The invariants are not optional: tools wrap the same validated ops a
person's edits use, and the loop persists nothing — a human accepts the
end state through the same path they always did. Guardrails live in the
harness: a step cap (ceiling 24), per-observation truncation, transcript
trimming, and a consecutive-failure limit.

Current consumers: the theme studio assistant (`theme-studio` purpose,
tools: inspect/edit/menus/edit_menu/look/content) and the page composer
(`page-designer`, tools: inspect/edit/look). Both show their steps in
the UI — the work, not just the answer.

**Menus (phase 58).** Navigation is part of the studio agent's design
surface: `menus` lists every menu with its links, `edit_menu` stages a
whole-menu rewrite (validated by the same `vyasa_core::menu` rules the
Menus page enforces), and `look` renders with the staged menus overlaid
so the agent sees the nav its proposal would produce. Staged drafts ride
the proposal's `menus` key and land through `MenuService::apply_draft`
only when the person accepts — the propose→apply boundary covers site
data too. An `edit` whose layout points at a menu nobody has gets a
note back, so "empty nav" is a fixable observation instead of a silent
miss.
