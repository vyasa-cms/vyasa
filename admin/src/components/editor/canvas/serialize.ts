/**
 * ProseMirror document ↔ `BlockDocument v1`.
 *
 * The wire format does not change: the canvas is a new *view* over the same
 * blocks the form editor writes, so posts open in either editor and no
 * migration is involved. Attribute names follow `../blocks.ts`, which
 * mirrors the server's validator and renderer.
 *
 * Design rule: **never drop a block.** Text-shaped kinds, images and tables
 * get a native node; every other kind round-trips verbatim through an
 * opaque `rpBlock` atom carrying its original `kind`, `attrs` and
 * `children`. The one exception is an image that has no file yet — it is
 * not content, and the server would reject it — so it is left out until an
 * upload completes.
 */
import type { Block, BlockKind } from "../blocks";
import { CONTAINER_KINDS } from "./extensions";
import { htmlToInline, inlineToHtml, type InlineNode } from "./inline";

export interface PMNode {
  type: string;
  attrs?: Record<string, unknown>;
  content?: PMNode[];
  text?: string;
  marks?: { type: string; attrs?: Record<string, unknown> }[];
}

/** Kinds the canvas edits directly; everything else becomes an `rpBlock`. */
export const NATIVE_KINDS: BlockKind[] = [
  "paragraph",
  "heading",
  "list",
  "quote",
  "code",
  "separator",
  "image",
  "table",
];

export function isNative(kind: BlockKind): boolean {
  return NATIVE_KINDS.includes(kind);
}

function str(attrs: Record<string, unknown>, key: string): string {
  const value = attrs[key];
  return typeof value === "string" ? value : "";
}

/** `attrs` minus the keys a native node models itself. */
function without(attrs: Record<string, unknown>, keys: string[]): Record<string, unknown> {
  const rest: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(attrs)) {
    if (!keys.includes(key)) rest[key] = value;
  }
  return rest;
}

/* ------------------------------------------------- blocks → ProseMirror ---- */

function inlineContent(html: string): PMNode[] {
  return htmlToInline(html) as unknown as PMNode[];
}

function paragraphFrom(html: string): PMNode {
  const content = inlineContent(html);
  return content.length === 0
    ? { type: "paragraph" }
    : { type: "paragraph", content };
}

function listToNode(block: Block): PMNode {
  const items: PMNode[] = [];
  for (const child of block.children ?? []) {
    if (child.kind === "list") {
      // A nested list belongs to the item before it. A list that opens with
      // a nested list gets an empty item to hang from, which is also what
      // the renderer produces.
      const nested = listToNode(child);
      const last = items[items.length - 1];
      if (last !== undefined) {
        (last.content ??= []).push(nested);
      } else {
        items.push({ type: "listItem", content: [{ type: "paragraph" }, nested] });
      }
      continue;
    }
    items.push({
      type: "listItem",
      content: [paragraphFrom(str(child.attrs, "text"))],
    });
  }
  return {
    type: block.attrs["ordered"] === true ? "orderedList" : "bulletList",
    content:
      items.length > 0
        ? items
        : [{ type: "listItem", content: [{ type: "paragraph" }] }],
  };
}

function tableToNode(block: Block): PMNode {
  const attrs = block.attrs;
  const header = Array.isArray(attrs["header"]) ? (attrs["header"] as unknown[]) : null;
  const rows = Array.isArray(attrs["rows"]) ? (attrs["rows"] as unknown[]) : [];
  const cell = (type: "tableHeader" | "tableCell", value: unknown): PMNode => ({
    type,
    content: [paragraphFrom(typeof value === "string" ? value : "")],
  });
  const width = Math.max(
    1,
    header?.length ?? 0,
    ...rows.map((r) => (Array.isArray(r) ? r.length : 0)),
  );
  const padded = (row: unknown[]): unknown[] =>
    row.length >= width ? row : [...row, ...Array<string>(width - row.length).fill("")];
  const content: PMNode[] = [];
  if (header !== null) {
    content.push({
      type: "tableRow",
      content: padded(header).map((c) => cell("tableHeader", c)),
    });
  }
  for (const row of rows) {
    const cells = Array.isArray(row) ? row : [];
    content.push({
      type: "tableRow",
      content: padded(cells).map((c) => cell("tableCell", c)),
    });
  }
  if (content.length === 0) {
    content.push({ type: "tableRow", content: [cell("tableCell", ""), cell("tableCell", "")] });
  }
  return {
    type: "table",
    attrs: {
      caption: str(attrs, "caption"),
      extra: without(attrs, ["header", "rows", "caption"]),
    },
    content,
  };
}

export function blockToNode(block: Block): PMNode {
  const attrs = block.attrs;

  switch (block.kind) {
    case "paragraph":
      return paragraphFrom(str(attrs, "text"));

    case "heading": {
      const raw = Number(attrs["level"] ?? 2);
      // The schema accepts 2..=6; the post title is the page's H1.
      const level = Number.isFinite(raw) ? Math.min(6, Math.max(2, raw)) : 2;
      const content = inlineContent(str(attrs, "text"));
      return content.length === 0
        ? { type: "heading", attrs: { level } }
        : { type: "heading", attrs: { level }, content };
    }

    case "list":
      return listToNode(block);

    case "quote":
      return {
        type: "blockquote",
        attrs: { citation: str(attrs, "citation") },
        content: [paragraphFrom(str(attrs, "text"))],
      };

    case "code": {
      const code = str(attrs, "code");
      const language = str(attrs, "language");
      return {
        type: "codeBlock",
        attrs: { language: language === "" || language === "plain" ? null : language },
        ...(code === "" ? {} : { content: [{ type: "text", text: code }] }),
      };
    }

    case "separator":
      return { type: "horizontalRule" };

    case "image": {
      const caption = inlineContent(str(attrs, "caption"));
      return {
        type: "vyImage",
        attrs: {
          url: str(attrs, "url"),
          alt: str(attrs, "alt"),
          extra: without(attrs, ["url", "alt", "caption"]),
        },
        ...(caption.length === 0 ? {} : { content: caption }),
      };
    }

    case "table":
      return tableToNode(block);

    default:
      if (CONTAINER_KINDS.includes(block.kind)) {
        // An editable region. Children are real blocks, recursively.
        const children = (block.children ?? []).map(blockToNode);
        return {
          type: "rpContainer",
          attrs: { kind: block.kind, attrs: block.attrs },
          content: children.length > 0 ? children : [{ type: "paragraph" }],
        };
      }
      // Opaque passthrough — the block survives untouched.
      return {
        type: "rpBlock",
        attrs: {
          kind: block.kind,
          attrs: block.attrs,
          children: block.children ?? [],
        },
      };
  }
}

export function blocksToDoc(blocks: Block[]): PMNode {
  const content = blocks.map(blockToNode);
  return {
    type: "doc",
    // ProseMirror requires at least one block; an empty post opens on a
    // paragraph ready to type into.
    content: content.length > 0 ? content : [{ type: "paragraph" }],
  };
}

/* ------------------------------------------------- ProseMirror → blocks ---- */

function inlineOf(node: PMNode): string {
  return inlineToHtml(node.content as unknown as InlineNode[] | undefined);
}

function textOf(node: PMNode): string {
  return (node.content ?? []).map((c) => c.text ?? "").join("");
}

function paragraph(text: string): Block {
  return { kind: "paragraph", attrs: { text }, children: [] };
}

function listToBlock(node: PMNode): Block {
  const children: Block[] = [];
  for (const item of node.content ?? []) {
    // An item's own text is its paragraphs joined by line breaks; any list
    // nested inside it follows as a sibling `list` block, which is how the
    // renderer expects nesting to arrive.
    const own: string[] = [];
    const nested: Block[] = [];
    for (const child of item.content ?? []) {
      if (child.type === "paragraph") own.push(inlineOf(child));
      else if (child.type === "bulletList" || child.type === "orderedList") {
        nested.push(listToBlock(child));
      }
    }
    children.push(paragraph(own.filter((t) => t !== "").join("<br>")));
    children.push(...nested);
  }
  return {
    kind: "list",
    attrs: { ordered: node.type === "orderedList" },
    children: children.length > 0 ? children : [paragraph("")],
  };
}

function tableToBlock(node: PMNode): Block | null {
  const rowNodes = node.content ?? [];
  const cellsOf = (row: PMNode): string[] =>
    (row.content ?? []).map((cell) =>
      (cell.content ?? [])
        .filter((c) => c.type === "paragraph")
        .map(inlineOf)
        .join("<br>"),
    );
  const isHeaderRow = (row: PMNode): boolean =>
    (row.content ?? []).length > 0 &&
    (row.content ?? []).every((cell) => cell.type === "tableHeader");

  const attrs: Record<string, unknown> = {
    ...((node.attrs?.["extra"] as Record<string, unknown> | undefined) ?? {}),
  };
  const first = rowNodes[0];
  let body = rowNodes;
  if (first !== undefined && isHeaderRow(first)) {
    attrs["header"] = cellsOf(first);
    body = rowNodes.slice(1);
  }
  attrs["rows"] = body.map(cellsOf);
  const caption = node.attrs?.["caption"];
  if (typeof caption === "string" && caption !== "") attrs["caption"] = caption;
  return { kind: "table", attrs, children: [] };
}

export function nodeToBlock(node: PMNode): Block | null {
  switch (node.type) {
    case "paragraph":
      return paragraph(inlineOf(node));

    case "heading": {
      const raw = Number(node.attrs?.["level"] ?? 2);
      const level = Number.isFinite(raw) ? Math.min(6, Math.max(2, raw)) : 2;
      return {
        kind: "heading",
        attrs: { level, text: inlineOf(node) },
        children: [],
      };
    }

    case "bulletList":
    case "orderedList":
      return listToBlock(node);

    case "blockquote": {
      const paragraphs = (node.content ?? []).filter(
        (c) => c.type === "paragraph",
      );
      const text = paragraphs.map(inlineOf).filter((t) => t !== "").join("<br>");
      const citation = node.attrs?.["citation"];
      return {
        kind: "quote",
        attrs: {
          text,
          citation: typeof citation === "string" ? citation : "",
        },
        children: [],
      };
    }

    case "codeBlock": {
      const language = node.attrs?.["language"];
      return {
        kind: "code",
        attrs: {
          language: typeof language === "string" && language !== "" ? language : null,
          code: textOf(node),
        },
        children: [],
      };
    }

    case "horizontalRule":
      return { kind: "separator", attrs: {}, children: [] };

    case "vyImage": {
      const url = node.attrs?.["url"];
      if (typeof url !== "string" || url === "") return null; // not content yet
      const alt = node.attrs?.["alt"];
      return {
        kind: "image",
        attrs: {
          ...((node.attrs?.["extra"] as Record<string, unknown> | undefined) ?? {}),
          url,
          alt: typeof alt === "string" ? alt : "",
          caption: inlineOf(node),
        },
        children: [],
      };
    }

    case "table":
      return tableToBlock(node);

    case "rpContainer": {
      const containerAttrs = node.attrs ?? {};
      const containerKind = containerAttrs["kind"];
      if (typeof containerKind !== "string") return null;
      const children = (node.content ?? [])
        .map(nodeToBlock)
        .filter((c): c is Block => c !== null);
      // A region the author emptied collapses to no children rather than
      // carrying a stray blank paragraph.
      const meaningful =
        children.length === 1 &&
        children[0]?.kind === "paragraph" &&
        children[0]?.attrs["text"] === ""
          ? []
          : children;
      return {
        kind: containerKind as BlockKind,
        attrs: (containerAttrs["attrs"] ?? {}) as Record<string, unknown>,
        children: meaningful,
      };
    }

    case "rpBlock": {
      const attrs = node.attrs ?? {};
      const kind = attrs["kind"];
      if (typeof kind !== "string") return null;
      return {
        kind: kind as BlockKind,
        attrs: (attrs["attrs"] ?? {}) as Record<string, unknown>,
        children: (Array.isArray(attrs["children"])
          ? attrs["children"]
          : []) as Block[],
      };
    }

    default:
      return null;
  }
}

export function docToBlocks(doc: PMNode): Block[] {
  const blocks = (doc.content ?? [])
    .map(nodeToBlock)
    .filter((b): b is Block => b !== null);
  // StarterKit keeps an empty paragraph after a trailing table, image or
  // container so there is always somewhere to click and type. It is a
  // convenience of the surface, not content, so it does not get saved.
  const last = blocks[blocks.length - 1];
  if (
    blocks.length > 1 &&
    last !== undefined &&
    last.kind === "paragraph" &&
    last.attrs["text"] === ""
  ) {
    return blocks.slice(0, -1);
  }
  return blocks;
}

/**
 * True when a document is a single empty paragraph — i.e. the author has not
 * written anything. Used so an empty canvas saves as zero blocks rather than
 * one blank paragraph.
 */
export function isEmptyDoc(doc: PMNode): boolean {
  const content = doc.content ?? [];
  if (content.length === 0) return true;
  if (content.length > 1) return false;
  const only = content[0];
  return (
    only !== undefined &&
    only.type === "paragraph" &&
    (only.content ?? []).length === 0
  );
}
