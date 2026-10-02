//! The stylesheet that makes design tokens visible.
//!
//! [`crate::css::tokens_to_css`] emits custom properties and nothing else, so
//! on its own a rendered page defines `--vy-color-bg` and friends but never
//! consumes them — the browser falls back to its default stylesheet and the
//! site renders as unstyled HTML. This module supplies the missing half: one
//! stylesheet of component rules for the classes the renderer emits, written
//! entirely in terms of `var(--vy-*)`.
//!
//! Because every value comes from a token, the same rules produce a different
//! design per theme; nothing here hard-codes a colour, size or family.

/// Rules consuming the token custom properties.
///
/// Almost static: every value comes from a custom property, so the rules
/// themselves never vary. The exception is the media queries, because CSS
/// cannot read a custom property inside `@media` — those two widths are
/// interpolated from the theme's own breakpoints, which is what stopped
/// them being magic numbers in this file.
#[must_use]
pub fn base_css(tokens: &crate::tokens::TokenSet) -> String {
    BASE_CSS
        .replace(SM_MARKER, &crate::css::px(tokens.layout.breakpoint_sm_px))
        .replace(MD_MARKER, &crate::css::px(tokens.layout.breakpoint_md_px))
}

/// Placeholders standing in for the theme's breakpoints in [`BASE_CSS`].
///
/// Spelled as widths that no theme may use (both are outside the accepted
/// range) so a stale marker shows up as an impossible query rather than a
/// plausible one.
const SM_MARKER: &str = "0sm";
const MD_MARKER: &str = "0md";

const BASE_CSS: &str = r"/* Vyasa base stylesheet — consumes the design tokens above. */
*, *::before, *::after { box-sizing: border-box; }

html { -webkit-text-size-adjust: 100%; }

body.vy {
  margin: 0;
  background: var(--vy-color-bg);
  color: var(--vy-color-text);
  font-family: var(--vy-font-body);
  font-size: var(--vy-base-size);
  line-height: calc(1.6 * var(--vy-density, 1));
  -webkit-font-smoothing: antialiased;
}

/* Layout ------------------------------------------------------------- */

.vy-header, .vy-main, .vy-footer-inner, .vy-nav {
  width: 100%;
  max-width: var(--vy-content-width);
  margin-inline: auto;
  padding-inline: calc(var(--vy-space-unit) * 5);
}

.vy-header {
  display: flex;
  align-items: baseline;
  gap: calc(var(--vy-space-unit) * 3);
  flex-wrap: wrap;
  padding-block: calc(var(--vy-space-unit) * 7);
  border-bottom: 1px solid var(--vy-color-border);
}

.vy-main { padding-block: calc(var(--vy-space-unit) * 8); }

.vy-footer {
  border-top: 1px solid var(--vy-color-border);
  margin-top: calc(var(--vy-space-unit) * 12);
  padding-block: calc(var(--vy-space-unit) * 6);
  color: var(--vy-color-text-muted);
  font-size: 0.9em;
}

.vy-sidebar, .vy-sidebar-left, .vy-sidebar-right {
  width: var(--vy-sidebar-width);
  flex: none;
}
.vy-sidebar:empty, .vy-sidebar-left:empty, .vy-sidebar-right:empty { display: none; }

/* Identity ----------------------------------------------------------- */

.vy-site-name {
  font-family: var(--vy-font-heading);
  font-size: calc(var(--vy-base-size) * var(--vy-scale-ratio) * 1.1);
  font-weight: 700;
  letter-spacing: -0.02em;
  color: var(--vy-color-text);
  text-decoration: none;
}
.vy-tagline { margin: 0; color: var(--vy-color-text-muted); font-size: 0.95em; }
.vy-tagline:empty { display: none; }

/* Navigation --------------------------------------------------------- */

.vy-nav, .vy-menu {
  display: flex;
  flex-wrap: wrap;
  gap: calc(var(--vy-space-unit) * 4);
  list-style: none;
  margin-block: 0;
  padding-block: calc(var(--vy-space-unit) * 3);
}
.vy-menu { margin-inline: 0; padding-inline: 0; }

/* Masthead: brand on the left, navigation on the right, one row. The
   header and nav keep their own rules for a template that places them
   apart; inside the masthead the wrapper owns width and the rule. */
.vy-masthead {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-wrap: wrap;
  gap: calc(var(--vy-space-unit) * 3) calc(var(--vy-space-unit) * 6);
  width: 100%;
  max-width: var(--vy-content-width);
  margin-inline: auto;
  padding-inline: calc(var(--vy-space-unit) * 5);
  border-bottom: 1px solid var(--vy-color-border);
}
.vy-masthead > .vy-header, .vy-masthead > .vy-nav {
  width: auto; max-width: none; margin: 0; padding-inline: 0; border: 0;
}
.vy-masthead > .vy-nav { padding-block: 0; margin-inline-start: auto; }
.vy-masthead:not(:has(.vy-nav)) { display: block; }
.vy-nav a, .vy-menu a {
  color: var(--vy-color-text-muted);
  text-decoration: none;
}
.vy-nav a:hover, .vy-menu a:hover { color: var(--vy-color-primary); }

/* Typography --------------------------------------------------------- */

.vy-h, .vy-title, .vy-archive-title {
  font-family: var(--vy-font-heading);
  line-height: 1.2;
  letter-spacing: -0.015em;
  margin: 0 0 calc(var(--vy-space-unit) * 3);
}
/* Fluid rather than a single step at the breakpoint. The upper bound is
   the previous fixed value, so wide screens render exactly as before and
   only narrow ones change. */
.vy-title, .vy-archive-title {
  font-size: clamp(
    calc(var(--vy-base-size) * var(--vy-scale-ratio) * 1.0),
    calc(var(--vy-base-size) * var(--vy-scale-ratio) * 0.72 + 2.2vw),
    calc(var(--vy-base-size) * var(--vy-scale-ratio) * 1.6)
  );
}
.vy-h { font-size: calc(var(--vy-base-size) * var(--vy-scale-ratio) * 1.1); }
.vy-h a, .vy-title a { color: inherit; text-decoration: none; }
.vy-h a:hover, .vy-title a:hover { color: var(--vy-color-primary); }

.vy-p, .vy-excerpt { margin: 0 0 calc(var(--vy-space-unit) * 4); }
.vy-excerpt:empty { display: none; }
.vy-meta, .vy-term-count {
  color: var(--vy-color-text-muted);
  font-size: 0.875em;
  margin: 0 0 calc(var(--vy-space-unit) * 2);
}

a { color: var(--vy-color-primary); }

/* Content blocks ----------------------------------------------------- */

.vy-single, .vy-page { max-width: 68ch; }
.vy-archive-header { display: grid; grid-template-columns: auto 1fr; column-gap: calc(var(--vy-space-unit) * 3); align-items: center; margin-block-end: calc(var(--vy-space-unit) * 5); }
.vy-archive-header .vy-archive-title { grid-column: 2; margin: 0; }
.vy-archive-image { grid-row: 1 / span 2; width: 72px; height: 72px; border-radius: 50%; object-fit: cover; }
.vy-archive-description { grid-column: 2; margin: 0; color: var(--vy-color-text-muted); }
.vy-single .vy-p, .vy-page .vy-p { line-height: calc(1.7 * var(--vy-density, 1)); }

.vy-list { padding-inline-start: calc(var(--vy-space-unit) * 6); }
.vy-list li { margin-block: calc(var(--vy-space-unit) * 1); }

.vy-quote {
  margin: calc(var(--vy-space-unit) * 6) 0;
  padding-inline-start: calc(var(--vy-space-unit) * 4);
  border-inline-start: 3px solid var(--vy-color-primary);
  color: var(--vy-color-text-muted);
  font-style: italic;
}
.vy-quote cite { display: block; margin-top: calc(var(--vy-space-unit) * 2); font-size: 0.875em; }

.vy-code, pre {
  background: var(--vy-color-surface);
  border: 1px solid var(--vy-color-border);
  border-radius: var(--vy-radius);
  padding: calc(var(--vy-space-unit) * 4);
  overflow-x: auto;
  font-size: 0.875em;
  line-height: 1.6;
}
code { font-size: 0.9em; }

.vy-image, .vy-video, .vy-audio, .vy-embed, .vy-cover, .vy-gallery {
  margin: calc(var(--vy-space-unit) * 6) 0;
}
.vy-image img, .vy-cover img, .vy-gallery img, .vy-media-text img, img {
  max-width: 100%;
  height: auto;
  border-radius: var(--vy-radius);
}
.vy-image figcaption, .vy-gallery figcaption {
  margin-top: calc(var(--vy-space-unit) * 2);
  color: var(--vy-color-text-muted);
  font-size: 0.85em;
  text-align: center;
}

.vy-separator {
  border: 0;
  border-top: 1px solid var(--vy-color-border);
  margin: calc(var(--vy-space-unit) * 8) 0;
}
.vy-page-break { display: none; }

/* The renderer wraps every table in a figure.vy-table, so the figure is
   the scroll box. `width: 100%` on a table is a floor for table
   layout, not a ceiling: past a certain number of columns the table is
   wider than its container no matter what, and without this it pushed the
   whole document sideways instead of scrolling on its own. */
.vy-table {
  max-width: 100%;
  overflow-x: auto;
  margin: calc(var(--vy-space-unit) * 6) 0;
}
/* The figure already carries the margin; the table inside must not add a
   second one. */
.vy-table > table { margin: 0; }
table {
  width: 100%;
  border-collapse: collapse;
  margin: calc(var(--vy-space-unit) * 6) 0;
  font-size: 0.95em;
}
.vy-table th, .vy-table td, table th, table td {
  border: 1px solid var(--vy-color-border);
  padding: calc(var(--vy-space-unit) * 2) calc(var(--vy-space-unit) * 3);
  text-align: start;
}
.vy-table th, table th { background: var(--vy-color-surface); font-weight: 600; }

.vy-button {
  display: inline-block;
  background: var(--vy-color-primary);
  color: var(--vy-color-on-primary);
  border-radius: var(--vy-radius);
  padding: calc(var(--vy-space-unit) * 2.5) calc(var(--vy-space-unit) * 5);
  text-decoration: none;
  font-weight: 600;
}
.vy-buttons { display: flex; flex-wrap: wrap; gap: calc(var(--vy-space-unit) * 3); }

.vy-details {
  border: 1px solid var(--vy-color-border);
  border-radius: var(--vy-radius);
  padding: calc(var(--vy-space-unit) * 3) calc(var(--vy-space-unit) * 4);
  margin: calc(var(--vy-space-unit) * 4) 0;
}
.vy-details summary { cursor: pointer; font-weight: 600; }

/* Callouts: a tinted box with a left rule in the tone's colour. The tones
   lean on the palette so they read in both modes. */
.vy-callout {
  border: 1px solid var(--vy-color-border);
  border-left: 4px solid var(--vy-color-primary);
  border-radius: var(--vy-radius);
  background: var(--vy-color-surface);
  padding: calc(var(--vy-space-unit) * 3) calc(var(--vy-space-unit) * 4);
  margin: calc(var(--vy-space-unit) * 4) 0;
}
.vy-callout > :last-child { margin-bottom: 0; }
.vy-callout__title { margin: 0 0 calc(var(--vy-space-unit) * 2); font-weight: 700; }
.vy-callout--tip { border-left-color: #2e8b57; }
.vy-callout--warning { border-left-color: #c8891a; }
.vy-callout--danger { border-left-color: #c0392b; }
.vy-toc { margin: calc(var(--vy-space-unit) * 4) 0; }
.vy-toc ol { list-style: none; margin: 0; padding: 0; }
.vy-toc li { margin: calc(var(--vy-space-unit) * 1) 0; }
.vy-toc__l3 { padding-left: calc(var(--vy-space-unit) * 4); }
.vy-toc__l4, .vy-toc__l5, .vy-toc__l6 { padding-left: calc(var(--vy-space-unit) * 8); }

.vy-file, .vy-embed-link { color: var(--vy-color-primary); }
.vy-footnotes { font-size: 0.85em; color: var(--vy-color-text-muted); }
.vy-html { margin: calc(var(--vy-space-unit) * 4) 0; }

/* Containers --------------------------------------------------------- */

.vy-group { margin: calc(var(--vy-space-unit) * 4) 0; }
.vy-row, .vy-columns, .vy-grid {
  display: grid;
  gap: calc(var(--vy-space-unit) * 5);
}
.vy-row, .vy-columns { grid-template-columns: repeat(2, minmax(0, 1fr)); }
.vy-grid { grid-template-columns: repeat(auto-fit, minmax(15rem, 1fr)); }
.vy-media-text {
  display: grid;
  gap: calc(var(--vy-space-unit) * 5);
  grid-template-columns: repeat(2, minmax(0, 1fr));
  align-items: center;
}
.vy-gallery { display: grid; grid-template-columns: repeat(auto-fit, minmax(12rem, 1fr)); }

/* Cards and listings ------------------------------------------------- */

.vy-post-list, .vy-latest-posts, .vy-archives, .vy-docs {
  display: grid;
  gap: calc(var(--vy-space-unit) * 6);
  list-style: none;
  margin: 0;
  padding: 0;
}

/* Documentation layout: the page tree beside the article. The docs
   starter theme has always emitted these class names; nothing matched
   them, so its two-column page rendered as one stacked column. */
@media (min-width: 0md) {
  .vy-docs {
    grid-template-columns: 15rem minmax(0, 1fr);
    align-items: start;
  }
}
.vy-docs__nav {
  font-size: 0.95em;
}
@media (min-width: 0md) {
  .vy-docs__nav {
    position: sticky;
    top: calc(var(--vy-space-unit) * 6);
    max-height: calc(100vh - var(--vy-space-unit) * 12);
    overflow-y: auto;
  }
}
.vy-docs__main { min-width: 0; }
.vy-docs__toc:not(:empty) {
  margin-bottom: calc(var(--vy-space-unit) * 6);
  padding: calc(var(--vy-space-unit) * 3) calc(var(--vy-space-unit) * 4);
  border-inline-start: 2px solid var(--vy-color-border);
  background: var(--vy-color-surface);
  border-radius: var(--vy-radius);
}

.vy-card, .vy-post-card {
  background: var(--vy-color-surface);
  border: 1px solid var(--vy-color-border);
  border-radius: var(--vy-radius);
  box-shadow: var(--vy-shadow);
  padding: calc(var(--vy-space-unit) * 5);
}

.vy-pagination {
  display: flex;
  gap: calc(var(--vy-space-unit) * 3);
  align-items: center;
  margin-top: calc(var(--vy-space-unit) * 8);
}
.vy-pagination a {
  border: 1px solid var(--vy-color-border);
  border-radius: var(--vy-radius);
  padding: calc(var(--vy-space-unit) * 2) calc(var(--vy-space-unit) * 4);
  text-decoration: none;
}

/* Comments ----------------------------------------------------------- */

.vy-comments { margin-top: calc(var(--vy-space-unit) * 10); }
.vy-comment-list { list-style: none; margin: 0; padding: 0; }
.vy-comment {
  border-top: 1px solid var(--vy-color-border);
  padding-block: calc(var(--vy-space-unit) * 4);
}
.vy-comment-children {
  list-style: none;
  margin-inline-start: calc(var(--vy-space-unit) * 6);
  padding-inline-start: calc(var(--vy-space-unit) * 4);
  border-inline-start: 1px solid var(--vy-color-border);
}
.vy-comments__title { margin-bottom: calc(var(--vy-space-unit) * 2); }
.vy-comments__empty, .vy-comment__meta {
  color: var(--vy-color-text-muted);
  font-size: 0.9em;
}
.vy-comment__body { margin-block: calc(var(--vy-space-unit) * 2); }
.vy-comment__actions { margin: 0; font-size: 0.85em; }

/* Comment form. The honeypot must be invisible to people: a bot fills
   every field it finds, a reader never sees this one. `display:none`
   rather than off-screen positioning, because assistive technology
   should not announce it either. */
.vy-comment-form {
  margin-top: calc(var(--vy-space-unit) * 8);
  padding-top: calc(var(--vy-space-unit) * 6);
  border-top: 1px solid var(--vy-color-border);
  max-width: 40rem;
}
.vy-comment-form__title { margin-bottom: calc(var(--vy-space-unit) * 3); }
.vy-comment-form__replying {
  color: var(--vy-color-text-muted);
  font-size: 0.9em;
}
.vy-field { display: block; margin-bottom: calc(var(--vy-space-unit) * 4); }
.vy-field label {
  display: block;
  margin-bottom: calc(var(--vy-space-unit) * 1);
  font-weight: 600;
  font-size: 0.9em;
}
.vy-field input, .vy-field textarea {
  display: block;
  width: 100%;
  box-sizing: border-box;
  background: var(--vy-color-surface);
  color: var(--vy-color-text);
  border: 1px solid var(--vy-color-border);
  border-radius: var(--vy-radius);
  padding: calc(var(--vy-space-unit) * 2) calc(var(--vy-space-unit) * 3);
  font: inherit;
}
.vy-field input:focus-visible, .vy-field textarea:focus-visible {
  outline: 2px solid var(--vy-color-primary);
  outline-offset: 1px;
}
.vy-field textarea { resize: vertical; min-height: 8em; }
.vy-field__hint {
  display: block;
  margin-top: calc(var(--vy-space-unit) * 1);
  color: var(--vy-color-text-muted);
  font-size: 0.85em;
}
.vy-field--trap { display: none !important; }
.vy-comment-form button {
  background: var(--vy-color-primary);
  color: var(--vy-color-on-primary);
  border: 0;
  border-radius: var(--vy-radius);
  padding: calc(var(--vy-space-unit) * 2.5) calc(var(--vy-space-unit) * 5);
  font: inherit;
  font-weight: 600;
  cursor: pointer;
}
.vy-comment-form button:hover { filter: brightness(1.08); }

/* The outcome of posting a comment, above the entry. */
.vy-notice {
  margin-bottom: calc(var(--vy-space-unit) * 5);
  padding: calc(var(--vy-space-unit) * 3) calc(var(--vy-space-unit) * 4);
  border: 1px solid var(--vy-color-border);
  border-inline-start: 3px solid var(--vy-color-primary);
  border-radius: var(--vy-radius);
  background: var(--vy-color-surface);
}

/* A post's categories and tags. */
.vy-terms {
  display: flex;
  flex-wrap: wrap;
  gap: calc(var(--vy-space-unit) * 2);
  margin-block: calc(var(--vy-space-unit) * 2);
}
.vy-term {
  font-size: 0.85em;
  padding: calc(var(--vy-space-unit) * 0.5) calc(var(--vy-space-unit) * 2);
  border: 1px solid var(--vy-color-border);
  border-radius: 999px;
  color: var(--vy-color-text-muted);
  text-decoration: none;
}
.vy-term:hover { color: var(--vy-color-primary); border-color: currentColor; }

/* Site logo in the header. */
.vy-logo { display: block; max-height: 3rem; width: auto; }
.vy-site-name { display: inline-flex; align-items: center; gap: .45em; }
.vy-logo-mark { height: 1.15em; width: auto; }

/* Supporting pages --------------------------------------------------- */

.vy-breadcrumbs, .vy-toc, .vy-docs-nav, .vy-categories, .vy-tag-cloud {
  color: var(--vy-color-text-muted);
  font-size: 0.9em;
}
.vy-toc, .vy-docs-nav { list-style: none; padding: 0; }
.vy-search input, .vy-password input {
  background: var(--vy-color-surface);
  color: var(--vy-color-text);
  border: 1px solid var(--vy-color-border);
  border-radius: var(--vy-radius);
  padding: calc(var(--vy-space-unit) * 2) calc(var(--vy-space-unit) * 3);
  font: inherit;
}
.vy-404 { text-align: center; padding-block: calc(var(--vy-space-unit) * 16); }

/* Search-result highlights. Derived from the theme's own primary rather
   than the browser default (black text on yellow), which is unreadable on a
   dark page. A browser without color-mix drops the declaration and falls
   back to that default, which is legible if not pretty. */
mark {
  background: color-mix(in srgb, var(--vy-color-primary) 20%, transparent);
  color: inherit;
  font-weight: 600;
  border-radius: 2px;
  padding: 0 0.15em;
}

/* Narrow screens ----------------------------------------------------- */

@media (max-width: 0sm) {
  .vy-row, .vy-columns, .vy-media-text { grid-template-columns: 1fr; }
  .vy-header { padding-block: calc(var(--vy-space-unit) * 5); }
  .vy-main { padding-block: calc(var(--vy-space-unit) * 6); }
  /* On a phone the nav dissolves into the masthead so the Menu button
     sits beside the brand and the opened list drops under both. */
  .vy-masthead > .vy-nav { display: contents; }
  .vy-masthead > .vy-nav > .vy-menu { flex-basis: 100%; }

  /* Touch targets, WCAG 2.5.8. Bare text links are the height of their
     line box -- 19px for body copy -- which is a coin-flip to hit with a
     thumb. Scoped to links that act as controls: navigation, list titles,
     cards, terms, pagination. Links inside prose are deliberately left
     alone; the guideline exempts them, and padding them would wreck the
     rhythm of a paragraph. */
  .vy-nav a, .vy-menu a, .vy-term, .vy-pagination a,
  .vy-comment__actions a, .vy-toc a, .vy-docs-nav a, .vy-file, .vy-embed-link,
  .vy-site-name, .vy-breadcrumbs a, .vy-categories a, .vy-tag-cloud a {
    display: inline-flex;
    align-items: center;
    min-height: 44px;
  }
  /* Titles wrap, so these grow by padding rather than by a line box: a
     fixed line-height would push the second line out of the box. */
  .vy-h a, .vy-title a, .vy-post-card__link {
    display: inline-block;
    min-height: 44px;
    padding-block: calc(var(--vy-space-unit) * 2);
  }
  .vy-search input, .vy-search button, .vy-comment-form button, .vy-btn, .vy-button {
    min-height: 44px;
  }
  .vy-search button { padding-inline: calc(var(--vy-space-unit) * 4); }
}

@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { transition-duration: 0.01ms !important; }
}
";
