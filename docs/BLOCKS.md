# Block wire format

The contract between the editor and the server. Every post's `content` is a
`BlockDocument`:

```json
{ "schema_version": 1, "blocks": [ { "kind": "…", "attrs": { … }, "children": [ … ] } ] }
```

`kind` picks the block type, `attrs` carries its properties, `children` holds
nested blocks for container kinds (omitted in responses when empty). The
server validates this shape in `crates/core/src/block/validate.rs` and
renders it in `crates/themes/src/renderer/blocks.rs`; the admin's copy is
`admin/src/components/editor/blocks.ts`. Those three files must agree on the
attribute names below — they once did not, and lists rendered empty while
buttons could not be saved.

Unknown attribute keys are preserved by the validator and ignored by the
renderer, so a plugin or a future editor can carry extra data without a
schema bump. Changing the meaning of a listed attribute needs one.

## Inline rich text

Fields marked *inline* hold an HTML string limited to the tags in
`vyasa_core::block::INLINE_TAGS`:

```
a b i em strong code br s del ins u mark sub sup
```

`a` keeps `href` and `title` only. The renderer sanitises with ammonia using
exactly this list, the WordPress importer filters to it, and the canvas
editor offers exactly the marks that map onto it (`b`→`strong`, `i`→`em`,
`del`→`s`, `ins`→`u`). Validation rejects `<script`, `<iframe`, `<style`,
`javascript:` and `onerror=` early; sanitisation at render is the security
boundary.

## Kinds

Required attributes are marked **(required)**; the validator rejects a block
without them.

### Text

| kind | attrs | children |
|---|---|---|
| `paragraph` | `text` inline | — |
| `heading` | `level` **(required)** 2–6 (the post title is the page's H1), `text` **(required)** inline | — |
| `list` | `ordered` bool (default false) | one child per item, each with `text` inline. A `list` child is a nested list and renders inside the item before it. |
| `quote` | `text` inline, `citation` inline (visible attribution), `cite` URL (the HTML `cite` attribute) | — |
| `code` | `code` **(required)** plain text, `language` identifier or null | — |
| `table` | `rows` **(required)** `string[][]` of inline cells, `header` `string[]` of inline cells (omit for no header row), `caption` inline | — |

### Media

Media blocks need either `url` (absolute `http(s)://` or root-relative `/…`,
which is how the library serves files: `/api/v1/media/{id}/raw`) or a numeric
`mediaId`. The renderer turns `mediaId` into that same library path.

| kind | attrs | children |
|---|---|---|
| `image` | `url`/`mediaId`, `alt`, `caption` inline | — |
| `gallery` | `caption` inline | `image` blocks only |
| `video` | `url`/`mediaId`, `caption` inline | — |
| `audio` | `url`/`mediaId`, `caption` inline | — |
| `file` | `url`/`mediaId`, `caption` inline — the link text; falls back to the file name, then "Download" | — |
| `cover` | `url`/`mediaId`, `text` inline (overlay) | any |
| `media_text` | `url`/`mediaId`, `alt`, `text` inline | any |

### Layout

| kind | attrs | children |
|---|---|---|
| `group`, `row`, `columns`, `grid` | `caption` inline | any |
| `buttons` | — | `button` blocks only |
| `button` | `label` **(required)** inline, `href` **(required)** http(s), root-relative, `#anchor` or `mailto:` | — |
| `details` | `summary` inline | any |
| `separator` | — | — |
| `page_break` | — | — |

### Special

| kind | attrs | children |
|---|---|---|
| `footnotes` | — | one child per note, each with `text` inline |
| `embed` | `url` **(required)** | — |
| `html` | `html` **(required)** — sanitised at render with a wider allowlist (no scripts, styles or handlers) | — |

### Structure (phase 72)

| kind | attrs | children | notes |
|---|---|---|---|
| `toc` | `depth` 2..6 (default 3) | — | Renders the document's headings as a list of anchors; nothing when there are none. |
| `callout` | `tone` note\|tip\|warning\|danger, `title` (inline HTML) | blocks | A tinted aside with a left rule in the tone's colour. |
| `timed` | `from`, `until` (RFC 3339) | blocks | Rendered only inside the window; nothing at all outside it. Rendered pages are cached, so a boundary shows through within the cache TTL. |

## Legacy shapes

An earlier admin build wrote `items`/`style` on lists, `images` on galleries,
`text`/`url` on buttons, `fileName` on files and `has_header` on tables.
`normalizeBlocks` in the admin rewrites those on load; nothing on the server
reads them. No production content was written in those shapes, so there is
no migration.

## Editors

Both admin editors read and write this format directly — the canvas
(`admin/src/components/editor/canvas/`) through a ProseMirror document it
serialises on every change, the classic form editor through per-kind fields.
Kinds the canvas does not model natively travel through it untouched.
