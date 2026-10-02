/**
 * The block model as the server defines it.
 *
 * This file is the admin's copy of the wire contract enforced by
 * `crates/core/src/block/validate.rs` and rendered by
 * `crates/themes/src/renderer/blocks.rs`. Attribute names here must match
 * those two files — the editor once wrote `items`/`style` for lists,
 * `images` for galleries and `text`/`url` for buttons while the renderer
 * read `ordered`+children, image children and `label`/`href`, so lists
 * rendered empty and buttons could not be saved at all. `docs/BLOCKS.md`
 * documents the canonical shape per kind; `normalizeBlocks` upgrades the
 * old shapes on read so nothing already written is lost.
 */

/**
 * A block kind a plugin contributed, always `namespace/name`.
 *
 * The slash is what keeps these from ever colliding with a core kind,
 * present or future — the server enforces the same rule in
 * `is_plugin_kind_name`.
 */
export type PluginBlockKind = `${string}/${string}`;

export type BlockKind =
  | PluginBlockKind
  | "paragraph"
  | "heading"
  | "list"
  | "quote"
  | "code"
  | "table"
  | "image"
  | "gallery"
  | "video"
  | "audio"
  | "file"
  | "cover"
  | "media_text"
  | "group"
  | "row"
  | "columns"
  | "grid"
  | "buttons"
  | "button"
  | "separator"
  | "page_break"
  | "details"
  | "footnotes"
  | "embed"
  | "html"
  | "toc"
  | "callout"
  | "timed"
  | "pattern"
  | "form";

export interface Block {
  kind: BlockKind;
  attrs: Record<string, unknown>;
  children: Block[];
}

export interface BlockDocument {
  schema_version: 1;
  blocks: Block[];
}

/** Whether this kind belongs to a plugin rather than the core registry. */
export function isPluginKind(kind: string): kind is PluginBlockKind {
  return kind.includes("/");
}

export const BLOCK_KINDS: { kind: BlockKind; label: string; icon: string }[] = [
  { kind: "paragraph", label: "Paragraph", icon: "¶" },
  { kind: "heading", label: "Heading", icon: "H" },
  { kind: "list", label: "List", icon: "≡" },
  { kind: "quote", label: "Quote", icon: "❝" },
  { kind: "code", label: "Code", icon: "</>" },
  { kind: "image", label: "Image", icon: "🖼" },
  { kind: "gallery", label: "Gallery", icon: "▦" },
  { kind: "video", label: "Video", icon: "▶" },
  { kind: "audio", label: "Audio", icon: "♫" },
  { kind: "file", label: "File", icon: "📎" },
  { kind: "table", label: "Table", icon: "⊞" },
  { kind: "cover", label: "Cover", icon: "⬒" },
  { kind: "media_text", label: "Media & Text", icon: "◫" },
  { kind: "buttons", label: "Buttons", icon: "⬢" },
  { kind: "button", label: "Button", icon: "⬣" },
  { kind: "separator", label: "Separator", icon: "—" },
  { kind: "columns", label: "Columns", icon: "▥" },
  { kind: "group", label: "Group", icon: "⬚" },
  { kind: "details", label: "Details", icon: "▸" },
  { kind: "embed", label: "Embed", icon: "◈" },
  { kind: "html", label: "Custom HTML", icon: "<>" },
  { kind: "page_break", label: "Page Break", icon: "↲" },
  { kind: "toc", label: "Table of contents", icon: "☰" },
  { kind: "callout", label: "Callout", icon: "❕" },
  { kind: "timed", label: "Timed", icon: "◷" },
  { kind: "pattern", label: "Synced pattern", icon: "⟳" },
  { kind: "form", label: "Form", icon: "✎" },
];

export const CALLOUT_TONES = ["note", "tip", "warning", "danger"] as const;

/** Media URL for a library item, in the form the public renderer serves. */
export function mediaUrl(id: string | number): string {
  return `/api/v1/media/${id}/raw`;
}

export function defaultAttrs(kind: BlockKind): Record<string, unknown> {
  switch (kind) {
    case "paragraph":
      return { text: "" };
    case "heading":
      return { level: 2, text: "" };
    case "list":
      return { ordered: false };
    case "quote":
      return { text: "", citation: "" };
    case "code":
      return { language: null, code: "" };
    case "table":
      return { header: ["", ""], rows: [["", ""]], caption: "" };
    case "image":
      return { url: "", alt: "", caption: "" };
    case "gallery":
      return {};
    case "video":
      return { url: "", caption: "" };
    case "audio":
      return { url: "", caption: "" };
    case "file":
      return { url: "", caption: "" };
    case "cover":
      return { url: "", text: "" };
    case "media_text":
      return { url: "", alt: "", text: "" };
    case "button":
      return { label: "Learn more", href: "/" };
    case "buttons":
      return {};
    case "separator":
      return {};
    case "page_break":
      return {};
    case "columns":
      return {};
    case "group":
    case "row":
    case "grid":
      return {};
    case "details":
      return { summary: "Details" };
    case "embed":
      return { url: "" };
    case "html":
      return { html: "" };
    case "toc":
      return { depth: 3 };
    case "callout":
      return { tone: "note", title: "" };
    case "timed":
      return { from: "", until: "" };
    case "pattern":
      return { pattern_id: "", name: "" };
    case "form":
      return { form_slug: "" };
    default:
      return {};
  }
}

/**
 * The children a freshly inserted block starts with. Lists need an item to
 * type into; containers open on an empty paragraph.
 */
export function defaultChildren(kind: BlockKind): Block[] {
  switch (kind) {
    case "list":
      return [createBlock("paragraph")];
    case "buttons":
      return [createBlock("button")];
    case "columns":
      return [createBlock("paragraph"), createBlock("paragraph")];
    case "group":
    case "row":
    case "grid":
    case "details":
    case "callout":
    case "timed":
      return [createBlock("paragraph")];
    default:
      return [];
  }
}

function asString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/**
 * Rewrites attribute shapes an earlier admin build produced into the ones
 * the server renders. Unknown keys are left alone — the validator preserves
 * them too — so this only touches the known divergences.
 */
function upgradeLegacy(kind: BlockKind, attrs: Record<string, unknown>, children: Block[]): {
  attrs: Record<string, unknown>;
  children: Block[];
} {
  const a = { ...attrs };
  switch (kind) {
    case "list": {
      if (Array.isArray(a["items"])) {
        const items = (a["items"] as unknown[]).map((item) => ({
          kind: "paragraph" as const,
          attrs: { text: asString(item) },
          children: [],
        }));
        delete a["items"];
        children = children.length > 0 ? children : items;
      }
      if ("style" in a) {
        a["ordered"] = a["style"] === "ordered";
        delete a["style"];
      }
      if (typeof a["ordered"] !== "boolean") a["ordered"] = false;
      return { attrs: a, children };
    }
    case "gallery": {
      if (Array.isArray(a["images"])) {
        const images = (a["images"] as unknown[]).map((raw) => {
          const im = (raw ?? {}) as Record<string, unknown>;
          return {
            kind: "image" as const,
            attrs: { url: asString(im["url"]), alt: asString(im["alt"]) },
            children: [],
          };
        });
        delete a["images"];
        children = children.length > 0 ? children : images;
      }
      return { attrs: a, children };
    }
    case "button": {
      if (!("label" in a) && "text" in a) {
        a["label"] = asString(a["text"]);
        delete a["text"];
      }
      if (!("href" in a) && "url" in a) {
        a["href"] = asString(a["url"]);
        delete a["url"];
      }
      return { attrs: a, children };
    }
    case "file": {
      if (!("caption" in a) && "fileName" in a) {
        a["caption"] = asString(a["fileName"]);
        delete a["fileName"];
      }
      return { attrs: a, children };
    }
    case "table": {
      const rows = Array.isArray(a["rows"]) ? (a["rows"] as unknown[][]) : [];
      if (a["has_header"] === true && !Array.isArray(a["header"]) && rows.length > 0) {
        a["header"] = rows[0];
        a["rows"] = rows.slice(1);
      }
      delete a["has_header"];
      return { attrs: a, children };
    }
    case "code": {
      const language = a["language"];
      if (language === "" || language === "plain" || language === "plaintext") {
        a["language"] = null;
      }
      return { attrs: a, children };
    }
    default:
      return { attrs: a, children };
  }
}

/**
 * Fills in fields the API omits and upgrades legacy attribute shapes.
 *
 * The server leaves `children` out of the JSON when a block has none, so code
 * that walks the tree cannot assume it is present — iterating `undefined`
 * throws. Normalising once on load keeps every consumer simple.
 */
export function normalizeBlocks(blocks: unknown): Block[] {
  if (!Array.isArray(blocks)) return [];
  return blocks.map((raw) => {
    const b = (raw ?? {}) as Partial<Block>;
    const kind = (b.kind ?? "paragraph") as BlockKind;
    const rawAttrs = b.attrs;
    const attrs =
      rawAttrs !== null && typeof rawAttrs === "object" && !Array.isArray(rawAttrs)
        ? (rawAttrs as Record<string, unknown>)
        : {};
    const upgraded = upgradeLegacy(kind, attrs, normalizeBlocks(b.children));
    return { kind, attrs: upgraded.attrs, children: upgraded.children };
  });
}

export function createBlock(kind: BlockKind): Block {
  return { kind, attrs: defaultAttrs(kind), children: defaultChildren(kind) };
}

/** Mirrors `BlockKind::is_container` on the server. */
export function isContainer(kind: BlockKind): boolean {
  return (
    kind === "group" ||
    kind === "row" ||
    kind === "columns" ||
    kind === "grid" ||
    kind === "buttons" ||
    kind === "details" ||
    kind === "gallery" ||
    kind === "quote" ||
    kind === "list" ||
    kind === "cover" ||
    kind === "media_text" ||
    kind === "table" ||
    kind === "callout" ||
    kind === "timed"
  );
}

/**
 * Text an author wrote, as plain words: inline HTML tags stripped, entities
 * decoded enough for counting. Walks every text-bearing attribute.
 */
export function plainTextOf(blocks: Block[]): string {
  const out: string[] = [];
  const visit = (list: Block[]) => {
    for (const block of list ?? []) {
      const attrs = block.attrs;
      for (const key of ["text", "caption", "citation", "summary", "label", "code"]) {
        const value = attrs[key];
        if (typeof value === "string" && value.trim() !== "") {
          out.push(stripTags(value));
        }
      }
      if (block.kind === "table") {
        for (const row of [attrs["header"], ...((attrs["rows"] as unknown[]) ?? [])]) {
          if (Array.isArray(row)) out.push(row.map(asString).join(" "));
        }
      }
      visit(block.children ?? []);
    }
  };
  visit(blocks);
  return out.join("\n");
}

export function stripTags(html: string): string {
  return html
    .replace(/<br\s*\/?>/gi, " ")
    .replace(/<[^>]+>/g, "")
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#x27;|&#39;/g, "'");
}

/** Words an author actually wrote. */
export function countWords(blocks: Block[]): number {
  const text = plainTextOf(blocks).trim();
  return text === "" ? 0 : text.split(/\s+/).length;
}

/** Minutes to read at an ordinary 200 words per minute, never below one. */
export function readingMinutes(words: number): number {
  return Math.max(1, Math.round(words / 200));
}

export interface OutlineEntry {
  level: number;
  text: string;
}

/** Headings in document order, for the outline panel. */
export function outlineOf(blocks: Block[]): OutlineEntry[] {
  const out: OutlineEntry[] = [];
  const visit = (list: Block[]) => {
    for (const block of list ?? []) {
      if (block.kind === "heading") {
        const raw = Number(block.attrs["level"] ?? 2);
        out.push({
          level: Number.isFinite(raw) ? Math.min(6, Math.max(2, raw)) : 2,
          text: stripTags(asString(block.attrs["text"])).trim() || "Untitled section",
        });
      }
      visit(block.children ?? []);
    }
  };
  visit(blocks);
  return out;
}
