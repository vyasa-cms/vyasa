# Vyasa Themes

Themes in Vyasa are **data, never executable code**: a token set, a
block layout tree, and (optionally) sandboxed Tera templates. This document
is the reference for the design-token schema (phase 20). Layout and
rendering are specified in phases 21–22.

## Design tokens v1 (`crates/themes`)

The canonical schema is `docs/theme-token-schema.json`, generated from the
Rust types. Regenerate after changing `TokenSet`:

```bash
cargo test -p vyasa-themes --test schema_export export_schema -- --ignored
```

A CI test (`committed_schema_is_current`) fails when the committed file is
stale.

### Document shape

```jsonc
{
  "version": 1,
  "colors": {
    "bg":         { "light": "#ffffff", "dark": "#101013" },
    "surface":    { "light": "#f4f4f5", "dark": "#1b1b1f" },
    "text":       { "light": "#18181b", "dark": "#f4f4f5" },
    "text_muted": { "light": "#52525b", "dark": "#a1a1aa" },
    "border":     { "light": "#e4e4e7", "dark": "#27272a" },
    "primary":    { "light": "#2563eb", "dark": "#60a5fa" },
    "on_primary": { "light": "#ffffff", "dark": "#0b1220" }
  },
  "typography": {
    "heading": "system_ui",          // system_ui | serif | mono | {"custom":"Inter"}
    "body": "system_ui",
    "base_size_px": 16,              // 12..=24
    "scale_ratio": "major_third",    // major_second|minor_third|major_third|perfect_fourth|golden|{"custom":1.0..2.5}
    "font_faces": [ { "family": "Inter", "src": "https://…/inter.woff2" } ]
  },
  "spacing": { "unit_px": 4, "section_scale": [1, 2, 4, 8] }, // ascending, ≤8 steps
  "radius_px": 8,                    // 0..=32
  "shadow": "small",                 // none|small|medium|large
  "layout": {
    "content_width_px": 960,         // 480..=1920
    "sidebar_width_px": 260,         // 160..=420, or 0 to disable
    "density": "comfortable",        // compact|comfortable|spacious
    "breakpoint_sm_px": 640,         // columns collapse, type steps down
    "breakpoint_md_px": 900          // sidebar drops under the content
  },
  "direction": "ltr"                 // ltr|rtl
}
```

Every field has a default — a document as small as `{"version": 1}` is
valid and equals the built-in neutral palette. Partial documents (e.g.
truncated AI output) are completed with defaults; unknown keys are always a
hard error so typos never disappear silently.

### Validation rules

| Kind | Where | Rule |
|---|---|---|
| error | *(document)* | Unknown key at any level (`deny_unknown_fields`) |
| error | `colors.*.{light,dark}` | Must match `#rgb` / `#rrggbb` |
| error | `version` | Must equal `1` |
| error | `typography.base_size_px` | `12..=24` |
| error | `typography.scale_ratio.custom` | `1.0..=2.5` |
| error | `typography.font_faces[].family` | Non-empty; `[A-Za-z0-9 _-]` only (CSS-injection guard) |
| error | `typography.font_faces[].src` | Must start with `https://` |
| error | `spacing.unit_px` | `2..=16` |
| error | `spacing.section_scale` | Non-empty, ≤ 8 steps, each `0.25..=20`, ascending |
| error | `radius_px` | `0..=32` |
| error | `layout.content_width_px` | `480..=1920` |
| error | `layout.sidebar_width_px` | `160..=420` or exactly `0` |
| error | `layout.breakpoint_{sm,md}_px` | `320..=1920`, and `sm` narrower than `md` |
| warning | color roles | WCAG AA contrast < 4.5 for `text/bg`, `text/surface`, `text_muted/surface`, `on_primary/primary` (checked per mode) |
| warning | `colors.<role>.dark` | Dark palette partially overridden; role keeps its light value |
| warning | `typography.font_faces[i]` | Duplicate family |
| warning | `typography.heading/body` | Custom family without a matching `@font-face` |

Errors block compilation; warnings are returned alongside valid CSS so the
admin token editor (phase 40) can surface them non-blockingly.

### CSS compilation

`tokens_to_css(&TokenSet) -> String` emits:

- one `:root` block with `--vy-color-*`, `--vy-radius`, `--vy-shadow`,
  `--vy-space-unit`, `--vy-section-scale`, `--vy-content-width`,
  `--vy-sidebar-width`, `--vy-density`, `--vy-direction`,
  `direction`, `--vy-base-size`, `--vy-scale-ratio`, `--vy-font-heading`,
  `--vy-font-body`;
- dark overrides under **both** `@media (prefers-color-scheme: dark)` and
  `[data-theme="dark"]` (manual toggle wins because it comes later);
- an `@font-face` rule per font face that declares `src`.

Output is deterministic (byte-identical for equal inputs), which lets the
public site cache compiled theme CSS by content hash.

```rust
let tokens = vyasa_themes::parse_token_set(json)?;
let (css, warnings) = vyasa_themes::compile_tokens(json)?;
```

## Composition

A layout is a **tree of sections**, one tree per template type. It used to
be a flat list of blog primitives with a single global palette, which could
stack things but never place them beside each other and never restyle one
band — so a hero above three columns above a dark call to action was simply
not expressible, however good the rest of the theme was.

```jsonc
{
  "id": "closing",          // unique across the whole template, [a-z0-9:_-]
  "kind": "band",           // a static region or a registered dynamic kind
  "settings": { },          // validated against the kind's schema
  "children": [ ],          // containers only
  "scope": { },             // optional colour override, see below
  "hide_on": "mobile"       // optional: "mobile" | "desktop"
}
```

**Containers and leaves.** `band`, `columns`, `grid` and `group` wrap
children and emit only their own wrapper; every other kind renders what its
resolver produced. Children on a leaf are an error rather than being
ignored, so a mistake surfaces at save time instead of as a missing
section. Nesting is capped at five levels.

**Screen size.** The two breakpoints are theme tokens. CSS cannot read a
custom property inside `@media`, so they are interpolated into the
generated stylesheets rather than emitted as variables — which is why they
live in the token document and not only in the CSS.

Sections respond to them two ways. `columns` takes `count`, `count_tablet`
(defaulting to half the desktop count, rounded up) and `count_mobile`. And
any section may set `"hide_on": "mobile"` or `"desktop"` to be withheld
from one size — a decorative band that is noise on a phone, or a
tap-to-call strip meant only for one. The two queries are exact
complements, so there is no width where a section is both hidden and shown.

**Style scopes.** A section may override colour roles for itself and
everything inside it:

```jsonc
"scope": { "bg": "$text", "text": "$bg", "padding_y": 8 }
```

A scoped section spans the page rather than the reading column — that is
what makes it a band and not a rectangle in the middle of the text — unless
it sets `"contained": true`. `band` bleeds on its own; the content inside
either stays at the reading width.

Each value is a hex literal or `$role` naming a theme colour. Prefer the
reference form: it keeps tracking the palette, so changing the theme's text
colour still changes the band, while a literal freezes today's value into
the layout forever. Scopes compile to a `.vy-scope-{id}` rule of custom
properties; because those inherit, a card inside a dark band is dark
without anyone reasoning about the cascade. Contrast is checked inside a
scope exactly as it is at the root.

### A page that composes itself

A page may carry its own section tree in `posts.layout`, in which case it
renders that instead of the theme's `page` template body. This is the
difference between a site where every page has one shape and a site where
one campaign can have a landing page.

- **Chrome still comes from the theme.** Header, nav and footer are placed
  by the template, so composing a page never means re-declaring the site.
  A `header` section inside a page tree is skipped rather than duplicated.
- **The author's writing is a section.** Give the tree a `content` (or
  `post-content`) section and the block document renders there — which is
  what lets prose sit *between* a hero and a call to action rather than
  only above or below them.
- **Ids cannot collide with the theme's.** A page's sections resolve in
  their own namespace, so a page section called `header` can never replace
  the site's.
- **Empty means "use the template".** `null` and `[]` both mean not
  composed, which is what every post and nearly every page is; clearing the
  tree puts a page back on its template without deleting anything else.
- **Only pages compose.** A composed body replaces the whole template body,
  and `single.html` is where the byline, the dated `<time>` and the taxonomy
  links live. None of those exist as section kinds yet, so a composed post
  would lose them with no way to put them back; the API refuses one until
  they do.

Written through `POST`/`PUT /api/v1/posts` as `layout`, validated against
the *active* theme's palette, revisioned with the content it arranges — so
reverting a page to Tuesday restores Tuesday's arrangement too, not last
night's sections around the restored text.

### Composing a page with the assistant

`POST /api/v1/posts/{id}/compose` takes `{message, content?, sections?}` and
answers `{reply, sections, warnings}`. **Nothing is stored**: the tree goes
back to the editor for the author to keep, undo, or edit, and is saved by
the ordinary post save — same validation, same permissions, same revisions.

The designer (`crates/ai/src/page_designer.rs`) is shown the vocabulary and
the palette its `$role` references resolve to, and is told what a page is:
sections replace this page's body, chrome comes from the theme, and the
author's writing belongs in a `content` section placed deliberately. A tree
that fails validation goes back to it with the diagnostics twice before the
request fails, so nothing invalid reaches an author.

`content` and `sections` in the request are the editor's *unsaved* state, so
the designer reads what the author is looking at rather than the last save.

`POST /api/v1/posts/{id}/render` takes the same two fields and answers with
the page's HTML, rendered through the **live** theme and storing nothing —
this is what the Sections view shows beside the tree, and what makes
composing something other than guesswork. The tree is validated before
rendering, so a bad kind is reported as `layout[0].kind` rather than
failing inside Tera.
`warnings` are the non-blocking diagnostics — mostly contrast — worth
showing because a band whose secondary text is unreadable validates
perfectly and only looks wrong once published.

## Bindings & per-type templates (phase 53)

**Bindings.** A repeating section can draw entries from a content source
instead of hand-entered items. The contract is a reserved `bind` object in
the section's settings:

```json
{ "id": "grid", "kind": "collection",
  "settings": { "bind": { "source": "product", "sort": "newest", "limit": 6 },
                "heading": "New arrivals", "columns": 3 } }
```

- `source` is a post type: `post`, `page`, a content type created in the
  admin, or anything a plugin registered through `register-post-types`.
  The live list is `GET /content-types`, and the vocabulary carries it as
  `sources` (with each source's fields).
- `sort` ∈ `newest | oldest | title | updated`, or `field:<key>` /
  `field:<key>:desc` to order by a custom field (entries without a value
  come last); `where` is an object of up to four `field: value` conditions,
  all of which must hold (a field holding several choices matches when it
  includes the value); `limit` is 1..=24;
  `term` filters by term slug (`"news"` = a category, `"shelf:fiction"`
  names another taxonomy).
- Only published entries of publicly-served types resolve. A source whose
  plugin is disabled renders as an **empty section, never an error** —
  the same survival rule stored entries follow. Unknown terms behave the
  same way: a deleted category must not 500 every page that mentions it.
- `collection` is the generic bound kind (standard cards: title, meta,
  cover, excerpt). Domain-aware cards — price badges, reply counts — are
  the owning plugin's business (phase 54 sections).

**Caching.** Bound sections render into cached pages like everything
else; a new entry appears when publishing invalidates the cache, which is
the existing invalidation contract. Do not "fix" this into per-request
queries — the cache is the reason the public site survives traffic.

**Per-type templates.** The renderer resolves `single-<type>.html` before
`single.html` and `archive-<type>.html` before `archive.html`, one level
of the WordPress hierarchy. A theme that ships `single-product.html`
renders every product through it — that is "design one product page, get
one per entry". Only the file resolves per type; the layout *tree* is
still the `single`/`archive` tree. Template names are validated by shape
(`studio::is_template_name`), so a template for a type that never exists
is dead weight, not an error.

## Content types and fields (phase 99)

An administrator creates content types (Products, Events, Team members…)
and gives any type — `post` and `page` included — its own fields, in the
admin's *Content types* page. Nothing needs a restart: a new type has
entries, `/<slug>/<entry>` URLs and (when enabled) an archive at `/<slug>`
on the next request, and `single-<slug>.html` / `archive-<slug>.html`
resolve for it exactly as for a plugin's type.

**`entry.fields`.** A single entry's template gets `entry` (`id`,
`title`, `type`, `fields`), so a value is `entry.fields.<key>`; in an
archive's `posts` loop each card carries them as `p.fields.<key>`:

```html
<p class="tagline">{{ entry.fields.tagline }}</p>
<p class="price">{{ entry.fields.price }}</p>
<a href="{{ entry.fields.website }}">Website</a>
<img src="{{ entry.fields.cover.url }}" alt="{{ entry.fields.cover.alt }}">
<a href="{{ entry.fields.author_page.url }}">{{ entry.fields.author_page.title }}</a>

{% for p in posts %}<li>{{ p.title }} — {{ p.fields.price }}</li>{% endfor %}
```

| Kind | In the template |
|---|---|
| `text`, `textarea`, `date` (`YYYY-MM-DD`) | a string |
| `number` | a number |
| `boolean` | `true` / `false` |
| `choice` | a string, or a list when several may be picked |
| `url` | a string: `http(s)://…` or a site-relative `/path` |
| `media` | `{ id, url, alt }` |
| `entry` | `{ id, title, url }` |

Rules a template can rely on:

- **Autoescaped, always.** Field values go through Tera's autoescaping like
  every other value; a field holding `<script>` renders as text. Nothing a
  field holds is ever rendered as raw HTML — do not reach for `| safe`.
- **Missing renders empty.** A key the type does not define (never did, or
  the field was deleted), or an entry with no value for it, renders as
  nothing — never an error page. This covers references written with
  dots — `entry.fields.<key>`, `p.fields.<key>`, `….<key>.url` — in the
  template and in what it extends, includes or imports, and only where
  the entry or card is there (a listing has no `entry`; guard with
  `{% if entry %}` as usual). Bracket access (`entry.fields["key"]`), a
  key held in a variable, and the `get` filter are not covered: give
  those a `| default(value="")` or an `is defined` test. The same holds
  for a binding that sorts or filters by a field that no longer exists:
  it matches nothing.
- **References that no longer resolve render nothing.** A `media` field
  whose item was deleted, or an `entry` field pointing at an entry that is
  deleted, trashed, not published, password-protected, or of a type that
  is not public, renders as empty — a draft's title never reaches the
  page.
- **URLs are rechecked at render time.** A `url` value is served only if it
  still passes the URL rule (`http://`, `https://`, or a single leading
  `/`; no `//`, `/\`, control, format or slash look-alike characters);
  otherwise it renders empty.
- Only fields the type still defines are served; values left behind by a
  deleted field stay stored (until an administrator cleans them up) but
  never reach a template.

The theme vocabulary lists each source's fields (`source_fields`) and the
per-type templates (`type_templates`), so the studio and the assistants
can compose with them. Text and textarea values are indexed for search and
listed in an entry's Markdown mirror and in `llms.txt`.

## Plugin sections (phase 54)

A plugin can register designer sections (`register-sections` /
`render-section` filter points — see docs/plugin-api.md). They join the
same registry the built-ins live in, so the insert library, the
inspector's schema-driven form, both assistants' vocabulary and the
renderer treat them identically. The host resolves any `bind` before
calling the plugin; the plugin only turns data into markup. A disabled
plugin's section warns in the studio, renders as an HTML comment on the
page, and survives in the stored layout — the rule everything
plugin-owned follows here.

## Designing a store, designing a forum (phase 55)

They are the same guide as any page — that is the point.

**A store:** enable the `storefront` plugin. Its sections appear in the
studio's insert library under *Commerce*, its `product` type in the Data
panel, and both in the assistants' vocabulary — "make me a store page
with the newest products" composes `store/product-grid` bound to
`product`. Products are ordinary entries; price and stock are meta set
through the plugin's route. The grid's binding is resolved by the host,
so drafts never leak onto the page.

**A forum:** enable `forum-lite`. Compose a page with
`forum/topic-list`, or just link the `/topic` archive. Replies are the
site's own comments — moderation, threading and notifications included —
and signed-in visitors start topics through the plugin's form.

Both survive their plugin being disabled: sections degrade to HTML
comments, entries stay stored, and re-enabling restores everything.

## Authoring a theme (the package, end to end)

A theme is a folder that becomes a zip. This is the whole shape; nothing
else is accepted at install, and a stray file is a hard error rather than
a warning so a package that installs here installs everywhere.

```text
my-theme/
├── manifest.toml        # name, version, author, required_api = 1
├── tokens.json          # colours (light + dark), type, spacing, layout
├── layout.json          # a section tree per template
├── templates/           # optional Tera overrides, flat, *.tera only
│   ├── single.tera      #   overrides single.html
│   ├── single-book.tera #   per-type: wins for the `book` post type
│   └── page.tera
├── assets/
│   ├── theme.css        # optional: anything the tokens cannot say
│   ├── theme.js         # optional: behaviour, served from this origin
│   ├── images/          # optional: pictures, at /theme-assets/images/…
│   │   ├── hero.jpg
│   │   └── icons/mark.svg
│   └── fonts/           # optional: web fonts, at /theme-assets/fonts/…
│       └── fraunces.woff2
└── screenshot.png       # optional, shown in the theme picker (≤ 1 MiB)
```

Package it with the CLI, which validates it as it zips:

```bash
vyasa theme pack my-theme                       # my-theme-1.vytheme
vyasa theme pack my-theme --key <secret hex>    # + my-theme-1.vytheme.sig
```

A theme that is only data uploads as it is. A theme that carries a
script — `assets/theme.js`, or a template that writes a `<script>` — runs
in every visitor's browser, so it must be signed: by the official
marketplace key when it comes from the marketplace, or by a key the
operator lists in `package_trusted_keys` when uploaded by hand (send the
`.sig` contents as the `signature` field beside `file`). `vyasa plugin
keygen` mints a key.

| Limit | Value |
|---|---|
| Package size | 25 MiB compressed |
| Entries | 200, of which at most 100 bundled files |
| One entry | 5 MiB uncompressed |
| Bundled files together | 20 MiB uncompressed |
| Screenshot | 1 MiB |
| Image types | png, jpg, gif, webp, avif, svg |
| Font types | woff2, woff, ttf, otf |
| Template names | `base`, `index`, `single`, `archive`, `page`, `search`, `not-found`, plus `<name>-<post-type>` |

**Where each layer stops.** Tokens describe the palette, type scale and
widths, and every section reads them, so a theme that only ships tokens
already restyles the whole site. The layout composes sections (hero,
columns, collection, feature grid, band, …) and scopes a band to its own
palette. Templates change the markup around the sections. `theme.css`
reaches whatever is left — the class names are stable and documented in
the base stylesheet — and is where animation lives: transitions, keyframes,
scroll-driven effects. The base stylesheet zeroes transitions under
`prefers-reduced-motion`, and a theme's own rules should respect the same
query.

**Bundled pictures and fonts.** `assets/images/` and `assets/fonts/`
travel with the package and are served from the site at
`/theme-assets/images/…` and `/theme-assets/fonts/…` while the theme is
active. Refer to them three ways: from `theme.css` by relative URL
(`url(images/hero.jpg)`, which resolves against the stylesheet's own
`/theme-assets/` URL); from `layout.json` as `/theme-assets/images/hero.jpg`
in any `src` or `media` setting; and from `tokens.json` as a
`font_faces[].src` of `/theme-assets/fonts/fraunces.woff2`. File names are
lowercase `[a-z0-9._-]`, at most one directory deep inside `images/` or
`fonts/`, and the extension decides the type — nothing is sniffed. SVG is
served with a policy that runs no script. The files are stored with the
theme version, copied forward when the studio publishes a new version,
and deleted with it; the render path never loads them. A theme still may
point at the media library or any `https://` host instead. On Appearance,
each installed version's *Files* button lists what it bundles and adds or
removes a picture or font in place; the same rules apply as at install.

**Script.** `theme.js` is not sandboxed. It is served from the site's own
origin, content-addressed, and versioned with the theme, and it is
accepted on the same trust that lets an administrator write site-wide
custom CSS. It is the right place for a menu animation or a lightbox and
the wrong place for anything a plugin should own.

**Two version numbers.** Every installed row has a `version`, the site's
own sequence (an install or a studio publish is the next number), and,
for rows that came from a package, a `package_version` from the
manifest. The marketplace's update check compares package versions, so
editing an installed theme in the studio never blocks its next update.

**Building on a starter.** The four starters under `themes-starter/` are
the reference packages: copy one, rename it in `manifest.toml`, and edit.
Every registered section kind renders in every starter on every template
(`crates/themes/tests/starter_matrix.rs` proves it), so a starter is a
safe base to grow from in the studio or by hand.

## Starter themes (phase 28)

Four packages ship embedded in the binary (`crates/api/src/
theme_defaults.rs`) and are auto-installed on first boot; **blog** is
activated automatically when no theme exists. Each stresses a different
part of the system:

| Theme | Exercises | Distinctive bits |
|---|---|---|
| `blog` | baseline | serif headings, sidebar-right, single.tera override |
| `portfolio` | gallery/grid-first, dark-only palette, zero sidebar (`sidebar_width_px: 0`), custom web font via @font-face, case-study single override |
| `docs` | page-tree nav (`docs-nav` dynblock), table of contents (`toc` block from heading anchors `#vy-h-N`), page.tera two-column override |
| `storefront-lite` | product listing via meta + price styling pattern, index.tera shop layout |

### Authoring checklist

1. Create `themes-starter/<name>/` with:
   - `manifest.toml` — `name` `[a-z0-9-]{1,60}`, integer `version >= 1`,
     `required_api = 1`.
   - `tokens.json` — see schema above; every field optional except
     `version`.
   - `layout.json` — all six templates must have ≥1 block; ids
     `[a-z0-9:_-]`, unique per template.
   - `templates/*.tera` (optional) — extends/includes limited to this
     package; compiled against the built-in set at install.
2. Package: zip the four files at archive root (no directory nesting).
3. Validate locally:
   ```bash
   cargo test -p vyasa-api theme_defaults   # embedded set sanity
   cargo test -p vyasa-themes package_tests # parser rules
   ```
4. Install on a running server:
   ```bash
   curl -F file=@theme.vytheme -F signature=@theme.vytheme.sig \
        -H "Cookie: $SESSION" http://localhost:8080/api/v1/themes
   ```

### Renderer conventions

- Heading blocks get deterministic anchor ids `vy-h-1..N`; build a TOC with
  `vyasa_themes::toc_from_blocks(&blocks)`.
- Region HTML from handlers is injected via `PageContext::regions_extra`
  and wins over layout-resolved blocks with the same id.
- Templates must pipe server-sanitized HTML through `| safe`
  (`regions.*`, `page.tokens_css`, `post.excerpt`).

## The theme studio (phase 42)

The studio at `/admin/appearance` edits a **working copy** — the same
three documents a `.vytheme` carries — and installs it as a theme version
when you publish. Nothing you do in the studio is visible to a visitor
until then.

### Why a draft

A `themes` row is immutable once installed: it is what the renderer
trusts, and its `(name, version)` is what the render cache is keyed on.
So the studio never edits one. `theme_drafts` (migration 0024) holds the
work in progress, `theme_draft_revisions` every step of it, and
`theme_draft_messages` the conversation with the assistant. Publishing
copies the draft into `themes` as `name@latest+1`; the draft stays, so
you can keep working and publish again.

### One edit path

Every change — a colour dragged in the inspector, a block reordered, a
template saved, or something the assistant proposed — is a `StudioOp`:

| Op | Effect | Validated by |
|---|---|---|
| `set_tokens` / `patch_tokens` | replace or merge-patch (RFC 7386) the token document | `parse_token_set` + `validate_token_set` |
| `set_layout` | replace one template's block list | `validate_layout` against `builtin_registry()` |
| `set_template` / `remove_template` | add or drop a Tera override | `Engine::install_theme_templates` (the sandbox) |
| `explain` | prose only, changes nothing | — |

`vyasa_themes::studio::apply` applies a batch, then validates the
**result as a whole** and returns either the new state with any
non-blocking warnings (contrast, incomplete dark palette) or every hard
diagnostic with its field path. A rejected batch changes nothing. This is
the only way a draft is written, which is what makes the assistant safe:
it has no privileged path, and no way to emit CSS or raw HTML.

Overrides are compiled **together**, so a template that includes a
sibling is checked against that sibling — matching what the renderer does
when it loads the theme.

### Vocabulary, not prose

`GET /api/v1/themes/vocabulary` returns the static regions, every
registered block kind with its settings JSON Schema, the seven template
names with their built-in sources, and the token JSON Schema — all
generated from `builtin_registry()` and `tokens::json_schema()`. The
layout editor's forms, and the assistant's system prompt, are both built
from it, so neither can drift from what the validator accepts.

### Preview

`POST /api/v1/themes/preview` renders documents that have not been saved,
over real published content, and never touches the cache. `path`
resolves the way the public router does — `/`, `/post/{slug}`,
`/{page}`, `/category/{slug}`, `/tag/{slug}`, `/search?s=`, anything else
falls through to the 404 template — so every template can be previewed.
`GET /preview/theme/{id}?path=` does the same for an installed version
and requires a session with `ManageThemes`.

A render failure comes back as 422 with the engine's message (naming the
template), which the studio shows above the preview rather than blanking
it.

### The assistant

Optional. With a default **text** model registered the studio gains its
assistant; with a **vision** model it also accepts images (a moodboard, a
screenshot) as reference. Without either, everything else in the studio
works unchanged.

On a screen wide enough the studio is three panes — conversation, live
preview, inspector — so the assistant is not somewhere you go instead of
the design. Below that it folds back into tabs beside the preview.

**It proposes; the author applies.** A reply carries the documents it would
write, and `POST /api/v1/themes/drafts/{id}/messages/{mid}/accept` is the
only thing that commits one. A proposal records the revision it was
composed against: accepting one built on an older draft is a 409 rather
than a silent overwrite, and accepting clears it so the same suggestion
cannot be replayed later.

### The brand kit

Durable context both assistants read before every generation, stored as the
`brand_kit` option and edited in Settings: `audience`, `voice`, `palette`,
`typography`, `references`, `avoid`. Each is free text, up to 600
characters; blank fields are dropped rather than sent as empty headings.

It exists because a model given only "a blog about Rust" produces the
median theme for that category. Given "lichen greens and bone, never blue"
and "a humanist sans throughout, no serifs" it produces that instead — and
the kit is stated in the prompt as taking precedence over any house style
of the model's own.

A request becomes an `ai_theme_chat` job (plans take longer than a
request should hold). The model is shown the vocabulary, the current
documents and the conversation, and answers with `{reply, ops}`; invalid
ops go back to it with the diagnostics, twice, before the run fails. A
run that changes something writes one revision attributed to
`assistant`, so undo treats it like any other step. Usage is priced and
logged in `ai_logs` under purpose `theme-studio`.
