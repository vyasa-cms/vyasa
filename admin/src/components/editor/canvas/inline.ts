/**
 * Inline rich text ↔ the HTML the block schema stores.
 *
 * The server sanitises inline fields through an ammonia allowlist that reads
 * `vyasa_core::block::INLINE_TAGS`: `a, b, i, em, strong, code, br, s, del,
 * ins, u, mark, sub, sup`. Anything else is stripped on render, so this
 * module round-trips exactly the marks that map onto those tags — the editor
 * must never offer formatting that disappears once the post is published.
 */

/** ProseMirror inline JSON, narrowed to what we produce and consume. */
export interface InlineMark {
  type: string;
  attrs?: Record<string, unknown>;
}

export interface InlineNode {
  type: "text" | "hardBreak";
  text?: string;
  marks?: InlineMark[];
}

const ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
};

function escapeHtml(value: string): string {
  return value.replace(/[&<>"]/g, (c) => ESCAPES[c] ?? c);
}

/** Marks are emitted outermost-first so nesting is stable across a round trip. */
const MARK_ORDER = [
  "link",
  "bold",
  "italic",
  "strike",
  "underline",
  "highlight",
  "subscript",
  "superscript",
  "code",
];

function openTag(mark: InlineMark): string {
  switch (mark.type) {
    case "bold":
      return "<strong>";
    case "italic":
      return "<em>";
    case "strike":
      return "<s>";
    case "underline":
      return "<u>";
    case "highlight":
      return "<mark>";
    case "subscript":
      return "<sub>";
    case "superscript":
      return "<sup>";
    case "code":
      return "<code>";
    case "link": {
      const href = String(mark.attrs?.["href"] ?? "");
      const title = mark.attrs?.["title"];
      const titleAttr =
        typeof title === "string" && title !== ""
          ? ` title="${escapeHtml(title)}"`
          : "";
      return `<a href="${escapeHtml(href)}"${titleAttr}>`;
    }
    default:
      return "";
  }
}

function closeTag(mark: InlineMark): string {
  switch (mark.type) {
    case "bold":
      return "</strong>";
    case "italic":
      return "</em>";
    case "strike":
      return "</s>";
    case "underline":
      return "</u>";
    case "highlight":
      return "</mark>";
    case "subscript":
      return "</sub>";
    case "superscript":
      return "</sup>";
    case "code":
      return "</code>";
    case "link":
      return "</a>";
    default:
      return "";
  }
}

function sortMarks(marks: InlineMark[]): InlineMark[] {
  return [...marks].sort(
    (a, b) => MARK_ORDER.indexOf(a.type) - MARK_ORDER.indexOf(b.type),
  );
}

function sameMark(a: InlineMark, b: InlineMark): boolean {
  return (
    a.type === b.type &&
    JSON.stringify(a.attrs ?? {}) === JSON.stringify(b.attrs ?? {})
  );
}

/**
 * Serialise ProseMirror inline content to the stored HTML string.
 *
 * Adjacent runs sharing a mark are wrapped once rather than per text node, so
 * `**bo** **ld**` typed as one bold run comes back as a single `<strong>`.
 */
export function inlineToHtml(content: InlineNode[] | undefined): string {
  if (content === undefined || content.length === 0) return "";

  let html = "";
  let open: InlineMark[] = [];

  const closeDownTo = (depth: number) => {
    while (open.length > depth) {
      const mark = open.pop();
      if (mark !== undefined) html += closeTag(mark);
    }
  };

  for (const node of content) {
    if (node.type === "hardBreak") {
      closeDownTo(0);
      html += "<br>";
      continue;
    }
    if (node.text === undefined || node.text === "") continue;

    const marks = sortMarks(node.marks ?? []).filter(
      (m) => MARK_ORDER.includes(m.type),
    );

    // Keep whatever prefix this node shares with the currently open marks.
    let shared = 0;
    while (
      shared < open.length &&
      shared < marks.length &&
      open[shared] !== undefined &&
      marks[shared] !== undefined &&
      sameMark(open[shared] as InlineMark, marks[shared] as InlineMark)
    ) {
      shared += 1;
    }
    closeDownTo(shared);
    for (let i = shared; i < marks.length; i += 1) {
      const mark = marks[i];
      if (mark === undefined) continue;
      html += openTag(mark);
      open.push(mark);
    }

    html += escapeHtml(node.text);
  }

  closeDownTo(0);
  open = [];
  return html;
}

const TAG_TO_MARK: Record<string, string> = {
  STRONG: "bold",
  B: "bold",
  EM: "italic",
  I: "italic",
  S: "strike",
  DEL: "strike",
  STRIKE: "strike",
  U: "underline",
  INS: "underline",
  MARK: "highlight",
  SUB: "subscript",
  SUP: "superscript",
  CODE: "code",
};

/**
 * Parse a stored HTML string back into ProseMirror inline content.
 *
 * Unknown elements contribute their text but not their tag, matching what the
 * server's sanitiser would have done anyway.
 */
export function htmlToInline(html: string): InlineNode[] {
  if (html === "") return [];

  // No DOM (a non-jsdom test runner): fall back to treating it as plain text
  // rather than throwing and losing the content.
  if (typeof DOMParser === "undefined") {
    return [{ type: "text", text: html }];
  }

  const doc = new DOMParser().parseFromString(
    `<body>${html}</body>`,
    "text/html",
  );
  const out: InlineNode[] = [];

  const walk = (node: Node, marks: InlineMark[]) => {
    for (const child of Array.from(node.childNodes)) {
      if (child.nodeType === 3) {
        const text = child.textContent ?? "";
        if (text !== "") {
          out.push(
            marks.length > 0
              ? { type: "text", text, marks: [...marks] }
              : { type: "text", text },
          );
        }
        continue;
      }
      if (child.nodeType !== 1) continue;

      const el = child as Element;
      if (el.tagName === "BR") {
        out.push({ type: "hardBreak" });
        continue;
      }
      if (el.tagName === "A") {
        const href = el.getAttribute("href") ?? "";
        const title = el.getAttribute("title");
        const attrs: Record<string, unknown> = { href };
        if (title !== null && title !== "") attrs["title"] = title;
        walk(el, [...marks, { type: "link", attrs }]);
        continue;
      }
      const markType = TAG_TO_MARK[el.tagName];
      walk(el, markType === undefined ? marks : [...marks, { type: markType }]);
    }
  };

  walk(doc.body, []);
  return out;
}

/** Plain text of an inline run — used for excerpts and word counts. */
export function inlineToText(content: InlineNode[] | undefined): string {
  if (content === undefined) return "";
  return content
    .map((n) => (n.type === "hardBreak" ? " " : (n.text ?? "")))
    .join("");
}
