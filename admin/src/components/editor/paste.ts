/**
 * Cleans pasted rich HTML before ProseMirror parses it.
 *
 * Google Docs is the main offender: it expresses bold/italic as
 * `<span style="font-weight:700">` (which the schema drops, silently
 * losing every emphasis in the paste) and wraps the whole clipboard in
 * `<b style="font-weight:normal" id="docs-internal-guid-…">` (which
 * makes the entire paste bold). Word and Notion add inert wrappers and
 * style/meta junk. This normalises all of it to the tags the editor's
 * schema actually models — strong/em/s — and unwraps the liars.
 */
export function cleanPastedHtml(html: string): string {
  if (typeof DOMParser === "undefined") return html;
  const doc = new DOMParser().parseFromString(html, "text/html");

  // Junk that should never reach the parser.
  for (const el of Array.from(doc.querySelectorAll("style, meta, title, script"))) {
    el.remove();
  }

  // Google Docs' clipboard wrapper: bold in name only.
  for (const el of Array.from(doc.querySelectorAll("b"))) {
    const weight = el.style.fontWeight;
    const isWrapper =
      el.id.startsWith("docs-internal-guid") || weight === "normal" || weight === "400";
    if (isWrapper) unwrap(el);
  }

  // Styled spans → real marks, innermost first so nesting survives.
  const spans = Array.from(doc.querySelectorAll("span")).reverse();
  for (const el of spans) {
    const style = el.style;
    const weight = Number.parseInt(style.fontWeight, 10);
    const bold = style.fontWeight === "bold" || (!Number.isNaN(weight) && weight >= 600);
    const italic = style.fontStyle === "italic";
    const struck = style.textDecoration.includes("line-through");
    let node: Element = el;
    if (bold) node = wrapInto(doc, node, "strong");
    if (italic) node = wrapInto(doc, node, "em");
    if (struck) wrapInto(doc, node, "s");
    unwrap(el);
  }

  return doc.body.innerHTML;
}

/** Replaces `el` with its children, in place. */
function unwrap(el: Element): void {
  const parent = el.parentNode;
  if (!parent) return;
  while (el.firstChild) parent.insertBefore(el.firstChild, el);
  parent.removeChild(el);
}

/** Wraps `el`'s position in a new `tag`, keeping `el` (and content) inside. */
function wrapInto(doc: Document, el: Element, tag: string): Element {
  const wrapper = doc.createElement(tag);
  el.replaceWith(wrapper);
  wrapper.appendChild(el);
  return wrapper;
}
