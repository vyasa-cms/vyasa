/**
 * Inline HTML ↔ a small Markdown for the classic editor's text fields.
 *
 * A paragraph is stored as inline HTML, and the classic editor edits the
 * stored field, so a link showed up as `<a href="/about">x</a>` in the box.
 * Authors write `**bold**`, `*italic*`, `` `code` ``, `~~struck~~` and
 * `[text](url)` instead; a line break is a line break. The mapping is
 * lossless: characters that would read as syntax are backslash-escaped on
 * the way out and honoured on the way in, and the few allowed tags this
 * syntax has no spelling for (`<u>`, `<mark>`, `<sub>`, `<sup>`, `<ins>`)
 * pass through as tags. Output uses the same tags the canvas writes, so a
 * document edited on both surfaces stays byte-stable.
 */

const RAW_TAGS = ["u", "mark", "sub", "sup", "ins"];
const RAW_TAG_RE = /^<(\/?)(u|mark|sub|sup|ins)>/i;

function escapeText(text: string): string {
  return text.replace(/[\\*`~[<]/g, (c) => `\\${c}`);
}

function escapeHtml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Stored inline HTML → the Markdown shown in a classic field. */
export function htmlToInlineMarkdown(html: string): string {
  if (html === "" || typeof DOMParser === "undefined") return html;
  const doc = new DOMParser().parseFromString(`<body>${html}</body>`, "text/html");
  const walk = (node: Node): string => {
    let out = "";
    for (const child of Array.from(node.childNodes)) {
      if (child.nodeType === 3) {
        out += escapeText(child.textContent ?? "");
        continue;
      }
      if (child.nodeType !== 1) continue;
      const el = child as Element;
      const tag = el.tagName.toLowerCase();
      switch (tag) {
        case "strong":
        case "b":
          out += `**${walk(el)}**`;
          break;
        case "em":
        case "i":
          out += `*${walk(el)}*`;
          break;
        case "s":
        case "del":
        case "strike":
          out += `~~${walk(el)}~~`;
          break;
        case "code":
          out += `\`${(el.textContent ?? "").replace(/`/g, "\\`")}\``;
          break;
        case "br":
          out += "\n";
          break;
        case "a": {
          const href = el.getAttribute("href") ?? "";
          out += `[${walk(el)}](${href.replace(/\)/g, "%29")})`;
          break;
        }
        default:
          if (RAW_TAGS.includes(tag)) out += `<${tag}>${walk(el)}</${tag}>`;
          else out += walk(el);
      }
    }
    return out;
  };
  return walk(doc.body);
}

/** The Markdown typed in a classic field → stored inline HTML. */
export function inlineMarkdownToHtml(md: string): string {
  let i = 0;
  let out = "";
  const open: string[] = [];
  const toggle = (mark: string, tag: string) => {
    const at = open.lastIndexOf(mark);
    if (at === -1) {
      open.push(mark);
      out += `<${tag}>`;
      return;
    }
    // Close everything opened after it too, so the HTML stays nested.
    while (open.length > at) {
      const m = open.pop();
      out += `</${m === "**" ? "strong" : m === "*" ? "em" : "s"}>`;
    }
  };
  while (i < md.length) {
    const c = md[i] as string;
    if (c === "\\" && i + 1 < md.length) {
      out += escapeHtml(md[i + 1] as string);
      i += 2;
      continue;
    }
    if (c === "`") {
      let j = i + 1;
      let code = "";
      while (j < md.length && md[j] !== "`") {
        if (md[j] === "\\" && md[j + 1] === "`") {
          code += "`";
          j += 2;
          continue;
        }
        code += md[j];
        j += 1;
      }
      if (j < md.length) {
        out += `<code>${escapeHtml(code)}</code>`;
        i = j + 1;
        continue;
      }
      out += "`";
      i += 1;
      continue;
    }
    if (md.startsWith("**", i)) {
      toggle("**", "strong");
      i += 2;
      continue;
    }
    if (md.startsWith("~~", i)) {
      toggle("~~", "s");
      i += 2;
      continue;
    }
    if (c === "*") {
      toggle("*", "em");
      i += 1;
      continue;
    }
    if (c === "[") {
      const close = findClosing(md, i);
      if (close !== -1 && md[close + 1] === "(") {
        const end = md.indexOf(")", close + 2);
        if (end !== -1) {
          const href = md.slice(close + 2, end).trim();
          out += `<a href="${escapeHtml(href).replace(/"/g, "&quot;")}">${inlineMarkdownToHtml(md.slice(i + 1, close))}</a>`;
          i = end + 1;
          continue;
        }
      }
      out += "[";
      i += 1;
      continue;
    }
    if (c === "\n") {
      out += "<br>";
      i += 1;
      continue;
    }
    if (c === "<") {
      const raw = RAW_TAG_RE.exec(md.slice(i));
      if (raw !== null) {
        out += `<${raw[1]}${(raw[2] as string).toLowerCase()}>`;
        i += raw[0].length;
        continue;
      }
    }
    out += escapeHtml(c);
    i += 1;
  }
  while (open.length > 0) {
    const m = open.pop();
    out += `</${m === "**" ? "strong" : m === "*" ? "em" : "s"}>`;
  }
  return out;
}

/** Index of the `]` matching the `[` at `from`, honouring escapes; -1 if none. */
function findClosing(md: string, from: number): number {
  let depth = 0;
  for (let i = from; i < md.length; i += 1) {
    if (md[i] === "\\") {
      i += 1;
      continue;
    }
    if (md[i] === "[") depth += 1;
    else if (md[i] === "]") {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/** Whether a stored field carries any markup this syntax can express. */
export function hasInlineMarkup(html: string): boolean {
  return /<(strong|b|em|i|s|del|code|br|a|u|mark|sub|sup|ins)\b/i.test(html);
}
