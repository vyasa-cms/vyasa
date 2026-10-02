import type { Block } from "./blocks";

/**
 * Inline-link insertion for the link suggester: wrap the first occurrence
 * of `phrase` in the document's text with `<a href="url">`, preserving the
 * text's own casing. Pure — the caller feeds the result back through the
 * editor's normal onChange path.
 *
 * Occurrences inside a tag (`<a href="…phrase…">`) or inside an existing
 * link's text are skipped: double-linking is worse than not linking.
 */
export function linkPhrase(blocks: Block[], phrase: string, url: string): Block[] | null {
  let done = false;
  const walk = (list: Block[]): Block[] =>
    list.map((b) => {
      if (done) return b;
      const next: Block = { ...b, attrs: { ...b.attrs } };
      const text = typeof b.attrs.text === "string" ? b.attrs.text : null;
      if (text !== null) {
        const wrapped = wrapFirst(text, phrase, url);
        if (wrapped !== null) {
          done = true;
          next.attrs = { ...next.attrs, text: wrapped };
          return next;
        }
      }
      if (b.children && b.children.length > 0) {
        next.children = walk(b.children);
      }
      return next;
    });
  const out = walk(blocks);
  return done ? out : null;
}

/** True when `index` sits inside `<…>` or inside an `<a …>…</a>` pair. */
function unsafeAt(html: string, index: number): boolean {
  const before = html.slice(0, index);
  const lastOpen = before.lastIndexOf("<");
  const lastClose = before.lastIndexOf(">");
  if (lastOpen > lastClose) return true; // inside a tag
  const lastAnchorOpen = before.toLowerCase().lastIndexOf("<a ");
  const lastAnchorClose = before.toLowerCase().lastIndexOf("</a>");
  return lastAnchorOpen > lastAnchorClose; // inside a link's text
}

function wrapFirst(html: string, phrase: string, url: string): string | null {
  const lower = html.toLowerCase();
  const needle = phrase.toLowerCase();
  let from = 0;
  for (;;) {
    const i = lower.indexOf(needle, from);
    if (i === -1) return null;
    if (!unsafeAt(html, i)) {
      const original = html.slice(i, i + phrase.length);
      const escapedUrl = url.replace(/"/g, "&quot;");
      return `${html.slice(0, i)}<a href="${escapedUrl}">${original}</a>${html.slice(i + phrase.length)}`;
    }
    from = i + 1;
  }
}

/**
 * A related-reading suggestion has no phrase to wrap; it lands as a new
 * closing paragraph instead.
 */
export function appendFurtherReading(blocks: Block[], title: string, url: string): Block[] {
  const esc = (t: string) =>
    t.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  return [
    ...blocks,
    {
      kind: "paragraph",
      attrs: { text: `Related: <a href="${esc(url)}">${esc(title)}</a>` },
      children: [],
    } as Block,
  ];
}
