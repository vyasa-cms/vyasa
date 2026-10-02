/**
 * Pasted Markdown → the HTML the canvas parses.
 *
 * A README or an Obsidian note pasted into the canvas landed as one plain
 * paragraph per line, headings and all. When a plain-text paste looks like
 * Markdown, it is converted here: headings, lists, quotes, fences, rules
 * and paragraphs, with inline marks through the same mapping the classic
 * fields use. Text that is not Markdown is left alone.
 */
import { inlineMarkdownToHtml } from "./inlineMarkdown";

const BLOCK_START = /^(#{1,6}\s|[-*+]\s|\d+[.)]\s|>\s?|```|---\s*$)/m;

/** Whether pasted text is worth treating as Markdown. */
export function looksLikeMarkdown(text: string): boolean {
  if (!text.includes("\n") && !/^#{1,6}\s/.test(text)) return false;
  return BLOCK_START.test(text) || /(\*\*|`|\[[^\]]+\]\([^)]+\))/.test(text);
}

function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Converts Markdown to HTML with the block kinds the canvas has. */
export function markdownToHtml(md: string): string {
  const lines = md.replace(/\r\n?/g, "\n").split("\n");
  const out: string[] = [];
  let i = 0;
  const para: string[] = [];
  const flush = () => {
    if (para.length > 0) {
      out.push(`<p>${inlineMarkdownToHtml(para.join("\n"))}</p>`);
      para.length = 0;
    }
  };
  while (i < lines.length) {
    const line = lines[i] as string;
    if (line.startsWith("```")) {
      flush();
      const lang = line.slice(3).trim();
      const code: string[] = [];
      i += 1;
      while (i < lines.length && !(lines[i] as string).startsWith("```")) {
        code.push(lines[i] as string);
        i += 1;
      }
      i += 1;
      const attr = lang === "" ? "" : ` class="language-${esc(lang)}"`;
      out.push(`<pre><code${attr}>${esc(code.join("\n"))}</code></pre>`);
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading !== null) {
      flush();
      // The post title is H1; a pasted H1 becomes the top section level.
      const hashes = (heading[1] as string).length;
      const level = Math.min(6, Math.max(2, hashes === 1 ? 2 : hashes));
      out.push(`<h${level}>${inlineMarkdownToHtml(heading[2] as string)}</h${level}>`);
      i += 1;
      continue;
    }
    if (/^(---|\*\*\*|___)\s*$/.test(line)) {
      flush();
      out.push("<hr>");
      i += 1;
      continue;
    }
    if (/^>\s?/.test(line)) {
      flush();
      const quote: string[] = [];
      while (i < lines.length && /^>\s?/.test(lines[i] as string)) {
        quote.push((lines[i] as string).replace(/^>\s?/, ""));
        i += 1;
      }
      out.push(`<blockquote><p>${inlineMarkdownToHtml(quote.join("\n"))}</p></blockquote>`);
      continue;
    }
    const item = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/.exec(line);
    if (item !== null) {
      flush();
      out.push(listFrom(lines, i, (n) => (i = n)));
      continue;
    }
    if (line.trim() === "") {
      flush();
      i += 1;
      continue;
    }
    para.push(line);
    i += 1;
  }
  flush();
  return out.join("\n");
}

/** Parses a run of list items (nested by indentation) starting at `start`. */
function listFrom(lines: string[], start: number, setIndex: (n: number) => void): string {
  interface Item { indent: number; ordered: boolean; text: string; children: Item[] }
  const roots: Item[] = [];
  const stack: Item[] = [];
  let i = start;
  while (i < lines.length) {
    const m = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/.exec(lines[i] as string);
    if (m === null) break;
    const item: Item = {
      indent: (m[1] as string).replace(/\t/g, "  ").length,
      ordered: /\d/.test(m[2] as string),
      text: m[3] as string,
      children: [],
    };
    while (stack.length > 0 && (stack[stack.length - 1] as Item).indent >= item.indent) stack.pop();
    const parent = stack[stack.length - 1];
    if (parent === undefined) roots.push(item);
    else parent.children.push(item);
    stack.push(item);
    i += 1;
  }
  setIndex(i);
  const render = (items: Item[]): string => {
    const tag = (items[0]?.ordered ?? false) ? "ol" : "ul";
    const body = items
      .map((it) => `<li><p>${inlineMarkdownToHtml(it.text)}</p>${it.children.length > 0 ? render(it.children) : ""}</li>`)
      .join("");
    return `<${tag}>${body}</${tag}>`;
  };
  return render(roots);
}
