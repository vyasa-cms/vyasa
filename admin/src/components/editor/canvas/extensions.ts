import { Extension, Node, mergeAttributes, type Editor } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import Blockquote from "@tiptap/extension-blockquote";
import Link from "@tiptap/extension-link";
import Placeholder from "@tiptap/extension-placeholder";
import Highlight from "@tiptap/extension-highlight";
import Subscript from "@tiptap/extension-subscript";
import Superscript from "@tiptap/extension-superscript";
import Typography from "@tiptap/extension-typography";
import { CodeBlockLowlight } from "@tiptap/extension-code-block-lowlight";
import { Table, TableCell, TableHeader, TableRow } from "@tiptap/extension-table";
import { ReactNodeViewRenderer } from "@tiptap/react";
import { BLOCK_KINDS } from "../blocks";
import { CodeBlockView } from "./CodeBlockView";
import { ImageView } from "./ImageView";
import { lowlight } from "./languages";

/**
 * Opaque carrier for every block kind the canvas does not edit natively —
 * galleries, embeds, plugin-supplied kinds. It holds the original `kind`,
 * `attrs` and `children` so a round trip is lossless, and renders as a
 * selectable card the author can reorder, duplicate or delete.
 */
export const RpBlock = Node.create({
  name: "rpBlock",
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  addAttributes() {
    return {
      kind: { default: "paragraph" },
      attrs: { default: {} },
      children: { default: [] },
    };
  },

  parseHTML() {
    return [{ tag: "div[data-vy-block]" }];
  },

  renderHTML({ HTMLAttributes }) {
    return [
      "div",
      mergeAttributes(HTMLAttributes, { "data-vy-block": "" }),
    ];
  },

  addNodeView() {
    return ({ node }) => {
      const kind = String(node.attrs["kind"] ?? "");
      const meta = BLOCK_KINDS.find((k) => k.kind === kind);

      const dom = document.createElement("div");
      dom.className =
        "my-2 flex items-center gap-3 rounded-lg border bg-muted/40 px-3 py-2.5 text-sm";
      dom.setAttribute("data-vy-block", kind);
      dom.contentEditable = "false";

      const icon = document.createElement("span");
      icon.className =
        "flex h-8 w-8 shrink-0 items-center justify-center rounded-md border bg-background font-mono text-xs text-muted-foreground";
      icon.textContent = meta?.icon ?? "?";
      icon.setAttribute("aria-hidden", "true");

      const body = document.createElement("span");
      body.className = "min-w-0 flex-1";

      const title = document.createElement("span");
      title.className = "block truncate font-medium";
      title.textContent = meta?.label ?? kind;

      const hint = document.createElement("span");
      hint.className = "block truncate text-xs text-muted-foreground";
      hint.textContent = summarise(kind, node.attrs["attrs"], node.attrs["children"]);

      body.append(title, hint);
      dom.append(icon, body);

      if (kind === "gallery") {
        const strip = galleryStrip(node.attrs["children"]);
        if (strip !== null) dom.append(strip);
      }
      return { dom };
    };
  },
});

/** One line describing an opaque block, so the card is not just a label. */
function summarise(kind: string, raw: unknown, children: unknown): string {
  const attrs = (raw ?? {}) as Record<string, unknown>;
  if (kind === "gallery") {
    const n = Array.isArray(children) ? children.length : 0;
    return n === 0 ? "No images yet" : `${n} ${n === 1 ? "image" : "images"}`;
  }
  if (kind === "page_break") return "Readers continue on the next page";
  if (kind === "html") {
    const html = attrs["html"];
    return typeof html === "string" && html.trim() !== ""
      ? html.trim().slice(0, 80)
      : "Empty — add markup in the panel";
  }
  for (const key of ["label", "summary", "title", "text", "caption"]) {
    const value = attrs[key];
    if (typeof value === "string" && value !== "") return value.slice(0, 80);
  }
  for (const key of ["url", "href"]) {
    const value = attrs[key];
    if (typeof value === "string" && value !== "") return value;
  }
  return "Edit in the block settings panel";
}

/** A row of thumbnails so a gallery card shows what it holds. */
function galleryStrip(children: unknown): HTMLElement | null {
  if (!Array.isArray(children) || children.length === 0) return null;
  const strip = document.createElement("span");
  strip.className = "flex shrink-0 gap-1";
  for (const child of children.slice(0, 4)) {
    const url = (child as { attrs?: { url?: unknown } })?.attrs?.url;
    if (typeof url !== "string" || url === "") continue;
    const img = document.createElement("img");
    img.src = url;
    img.alt = "";
    img.loading = "lazy";
    img.className = "h-8 w-8 rounded border object-cover";
    strip.append(img);
  }
  return strip.childElementCount > 0 ? strip : null;
}

/**
 * Layout kinds that genuinely hold other blocks. These become editable
 * regions rather than opaque cards, so a two-column layout can be written
 * into directly instead of being a placeholder you cannot open.
 */
export const CONTAINER_KINDS = [
  "group",
  "row",
  "columns",
  "grid",
  "buttons",
  "details",
  "callout",
  "timed",
];

/** What a container's label says beyond its kind: the tone, the window. */
function containerHint(kind: string, attrs: unknown): string {
  const a = (attrs ?? {}) as Record<string, unknown>;
  if (kind === "callout") {
    const tone = typeof a["tone"] === "string" ? a["tone"] : "note";
    const title = typeof a["title"] === "string" && a["title"] !== "" ? ` · ${a["title"]}` : "";
    return `${tone}${title}`;
  }
  if (kind === "timed") {
    const fmt = (v: unknown) => (typeof v === "string" && v !== "" ? new Date(v).toLocaleString() : "…");
    return `${fmt(a["from"])} → ${fmt(a["until"])}`;
  }
  return "";
}

/** A nestable region. Its children are ordinary blocks in the same document. */
export const RpContainer = Node.create({
  name: "rpContainer",
  group: "block",
  content: "block+",
  defining: true,

  addAttributes() {
    return {
      kind: { default: "group" },
      attrs: { default: {} },
    };
  },

  parseHTML() {
    return [{ tag: "div[data-vy-container]" }];
  },

  renderHTML({ HTMLAttributes }) {
    return [
      "div",
      mergeAttributes(HTMLAttributes, { "data-vy-container": "" }),
      0,
    ];
  },

  addNodeView() {
    return ({ node }) => {
      const kind = String(node.attrs["kind"] ?? "");
      const meta = BLOCK_KINDS.find((k) => k.kind === kind);

      const dom = document.createElement("div");
      dom.className = "vy-container";
      dom.setAttribute("data-vy-container", kind);

      const label = document.createElement("div");
      label.className = "vy-container-label";
      label.contentEditable = "false";
      const hint = containerHint(kind, node.attrs["attrs"]);
      label.textContent = hint === "" ? (meta?.label ?? kind) : `${meta?.label ?? kind} · ${hint}`;
      if (kind === "callout") {
        const tone = ((node.attrs["attrs"] ?? {}) as Record<string, unknown>)["tone"];
        dom.setAttribute("data-vy-tone", typeof tone === "string" ? tone : "note");
      }

      const content = document.createElement("div");
      content.className = "vy-container-content";

      dom.append(label, content);
      return {
        dom,
        contentDOM: content,
        // Attribute edits from the settings panel re-label in place rather
        // than rebuilding the view and dropping the caret inside it.
        update: (next) => {
          if (next.type.name !== "rpContainer" || next.attrs["kind"] !== kind) return false;
          const h = containerHint(kind, next.attrs["attrs"]);
          label.textContent = h === "" ? (meta?.label ?? kind) : `${meta?.label ?? kind} · ${h}`;
          if (kind === "callout") {
            const tone = ((next.attrs["attrs"] ?? {}) as Record<string, unknown>)["tone"];
            dom.setAttribute("data-vy-tone", typeof tone === "string" ? tone : "note");
          }
          return true;
        },
      };
    };
  },
});

/**
 * An image as an image: rendered in place, caption written underneath,
 * uploaded by dropping or pasting a file. `pending` holds a local object URL
 * while an upload is in flight and never reaches the wire format.
 */
export const VyImage = Node.create({
  name: "vyImage",
  group: "block",
  content: "inline*",
  draggable: true,
  selectable: true,
  isolating: true,

  addAttributes() {
    return {
      url: { default: "" },
      alt: { default: "" },
      /** Any other attributes the block carried, kept for a lossless save. */
      extra: { default: {} },
      pending: { default: null, rendered: false },
      uploadId: { default: null, rendered: false },
    };
  },

  parseHTML() {
    return [{ tag: "figure[data-vy-image]" }];
  },

  renderHTML({ HTMLAttributes }) {
    return [
      "figure",
      mergeAttributes(HTMLAttributes, { "data-vy-image": "" }),
      ["img", { src: HTMLAttributes["url"], alt: HTMLAttributes["alt"] }],
      ["figcaption", 0],
    ];
  },

  addNodeView() {
    return ReactNodeViewRenderer(ImageView);
  },
});

/**
 * Quotes carry an optional citation. ProseMirror discards attributes a node
 * does not declare, so without this the attribution on every existing quote
 * would be dropped the first time the canvas saved it.
 */
const QuoteWithCitation = Blockquote.extend({
  addAttributes() {
    return {
      citation: {
        default: "",
        parseHTML: (element) => element.getAttribute("data-citation") ?? "",
        renderHTML: (attributes) => {
          const citation = attributes["citation"];
          return typeof citation === "string" && citation !== ""
            ? { "data-citation": citation }
            : {};
        },
      },
    };
  },
});

const CodeBlock = CodeBlockLowlight.extend({
  addNodeView() {
    return ReactNodeViewRenderer(CodeBlockView);
  },
}).configure({
  lowlight,
  defaultLanguage: null,
  // The picker sets the language; guessing from the fence text is a
  // markdown-paste feature this editor does not have.
  enableTabIndentation: true,
  tabSize: 2,
});

/** Name of the DOM event the link shortcut raises on the editor element. */
export const LINK_PROMPT_EVENT = "vy:link-prompt";

/**
 * Asks for a link. The bubble toolbar owns the prompt, so this only widens
 * the selection (to the word under the caret, or the whole existing link)
 * and raises a DOM event the toolbar listens for. Shared by Ctrl/Cmd+K and
 * the phone toolbar's link button.
 */
export function promptLink(editor: Editor): boolean {
  if (editor.isActive("codeBlock")) return false;
  const { empty } = editor.state.selection;
  if (empty) {
    if (editor.isActive("link")) {
      editor.chain().extendMarkRange("link").run();
    } else if (!selectWord(editor)) {
      return false;
    }
  }
  editor.view.dom.dispatchEvent(new CustomEvent(LINK_PROMPT_EVENT));
  return true;
}

const LinkShortcut = Extension.create({
  name: "vyLinkShortcut",
  addKeyboardShortcuts() {
    return {
      "Mod-k": () => {
        promptLink(this.editor);
        return true; // swallowed either way: the browser's Ctrl+K is the address bar
      },
    };
  },
});

/**
 * Tables carry a caption and any attributes the block had that the table
 * nodes do not model, so a round trip through the canvas loses nothing.
 */
const VyTable = Table.extend({
  addAttributes() {
    return {
      ...this.parent?.(),
      caption: { default: "", rendered: false },
      extra: { default: {}, rendered: false },
    };
  },
});

/** Selects the word around the caret. False when the caret is not on one. */
function selectWord(editor: Editor): boolean {
  const { $from } = editor.state.selection;
  const text = $from.parent.textContent;
  const offset = $from.parentOffset;
  const isWord = (c: string | undefined) => c !== undefined && /[\p{L}\p{N}_'-]/u.test(c);
  let start = offset;
  let end = offset;
  while (start > 0 && isWord(text[start - 1])) start -= 1;
  while (end < text.length && isWord(text[end])) end += 1;
  if (start === end) return false;
  const base = $from.start();
  editor.commands.setTextSelection({ from: base + start, to: base + end });
  return true;
}

/**
 * The editor's extension set.
 *
 * Every mark here maps onto a tag in `vyasa_core::block::INLINE_TAGS`; see
 * ./inline.ts. Offering formatting the server would strip is worse than not
 * offering it, so the schema test in `canvas_schema.test.ts` pins the list.
 */
export function buildExtensions() {
  return [
    StarterKit.configure({
      heading: { levels: [2, 3, 4, 5, 6] }, // H1 is the post title
      link: false, // configured separately below
      blockquote: false, // replaced by QuoteWithCitation
      codeBlock: false, // replaced by the highlighted block
      // Keep the history, list keymap and input rules StarterKit brings;
      // they give us undo/redo and Markdown shortcuts for free.
    }),
    Link.configure({
      openOnClick: false,
      autolink: true,
      linkOnPaste: true,
      protocols: ["http", "https", "mailto"],
      HTMLAttributes: { rel: "noopener noreferrer" },
    }),
    Highlight,
    Subscript,
    Superscript,
    Typography,
    QuoteWithCitation,
    CodeBlock,
    VyTable.configure({ resizable: false, HTMLAttributes: { class: "vy-table" } }),
    TableRow,
    TableHeader,
    TableCell,
    VyImage,
    Placeholder.configure({
      placeholder: ({ node }) =>
        node.type.name === "heading"
          ? "Heading"
          : node.type.name === "vyImage"
            ? "Write a caption (optional)"
            : "Write something, or press / for blocks",
    }),
    LinkShortcut,
    RpBlock,
    RpContainer,
  ];
}
