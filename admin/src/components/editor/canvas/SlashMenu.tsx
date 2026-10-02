import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import type { Editor } from "@tiptap/core";
import { BLOCK_KINDS, createBlock, type BlockKind } from "../blocks";
import { CONTAINER_KINDS } from "./extensions";
import { blockToNode, isNative } from "./serialize";
import { runContinue } from "./ai";
import { getPluginSurface } from "@/api/plugins";
import { api, type Pattern } from "@/api/client";
import type { Block } from "../blocks";
import { useAiAvailable } from "../useAiAvailable";
import { cn } from "@/lib/utils";

interface Item {
  key: string;
  label: string;
  hint: string;
  group: string;
  icon: string;
  keywords: string[];
  run: (editor: Editor) => unknown;
}

/**
 * Insert a block for a kind the canvas doesn't edit natively. Layout kinds
 * become editable regions; everything else is an opaque card edited in the
 * settings panel.
 */
function insertKind(kind: BlockKind) {
  return (editor: Editor) => {
    const block = createBlock(kind);
    const node = CONTAINER_KINDS.includes(kind)
      ? blockToNode(block)
      : { type: "rpBlock", attrs: { kind, attrs: block.attrs, children: block.children } };
    editor
      .chain()
      .focus()
      .insertContent(node as unknown as Record<string, unknown>)
      .run();
  };
}

const TEXT_ITEMS: Item[] = [
  {
    key: "paragraph",
    label: "Text",
    hint: "Plain paragraph",
    group: "Text",
    icon: "¶",
    keywords: ["paragraph", "text", "body"],
    run: (e) => e.chain().focus().setParagraph().run(),
  },
  ...[2, 3, 4].map((level) => ({
    key: `heading-${level}`,
    label: `Heading ${level}`,
    hint: `Section heading, level ${level}`,
    group: "Text",
    icon: `H${level}`,
    keywords: ["heading", "title", `h${level}`],
    run: (e: Editor) =>
      e.chain().focus().toggleHeading({ level: level as 2 | 3 | 4 }).run(),
  })),
  {
    key: "bulletList",
    label: "Bulleted list",
    hint: "An unordered list",
    group: "Text",
    icon: "•",
    keywords: ["list", "bullet", "unordered", "ul"],
    run: (e) => e.chain().focus().toggleBulletList().run(),
  },
  {
    key: "orderedList",
    label: "Numbered list",
    hint: "An ordered list",
    group: "Text",
    icon: "1.",
    keywords: ["list", "number", "ordered", "ol"],
    run: (e) => e.chain().focus().toggleOrderedList().run(),
  },
  {
    key: "blockquote",
    label: "Quote",
    hint: "Quoted passage with attribution",
    group: "Text",
    icon: "❝",
    keywords: ["quote", "blockquote", "citation"],
    run: (e) => e.chain().focus().toggleBlockquote().run(),
  },
  {
    key: "codeBlock",
    label: "Code",
    hint: "Highlighted, with a language picker",
    group: "Text",
    icon: "</>",
    keywords: ["code", "pre", "snippet"],
    run: (e) => e.chain().focus().toggleCodeBlock().run(),
  },
  {
    key: "separator",
    label: "Divider",
    hint: "Horizontal rule",
    group: "Text",
    icon: "—",
    keywords: ["divider", "separator", "hr", "rule"],
    run: (e) => e.chain().focus().setHorizontalRule().run(),
  },
  {
    key: "image",
    label: "Image",
    hint: "Upload, pick from the library, or paste a link",
    group: "Media",
    icon: "🖼",
    keywords: ["image", "photo", "picture", "figure", "upload"],
    run: (e) =>
      e
        .chain()
        .focus()
        .insertContent({ type: "vyImage", attrs: { url: "", alt: "" } })
        .run(),
  },
  {
    key: "table",
    label: "Table",
    hint: "Three by three, with a header row",
    group: "Layout",
    icon: "⊞",
    keywords: ["table", "grid", "rows", "columns"],
    run: (e) =>
      e.chain().focus().insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run(),
  },
  {
    key: "continue",
    label: "Continue writing",
    hint: "The assistant adds the next paragraph",
    group: "Assist",
    icon: "✦",
    keywords: ["ai", "continue", "write", "assist", "next"],
    run: (e) => runContinue(e),
  },
];

const GROUP_OF: Record<string, string> = {
  gallery: "Media",
  video: "Media",
  audio: "Media",
  file: "Media",
  cover: "Media",
  media_text: "Media",
  columns: "Layout",
  group: "Layout",
  row: "Layout",
  grid: "Layout",
  buttons: "Layout",
  button: "Layout",
  details: "Layout",
  page_break: "Layout",
  embed: "Embed",
  html: "Embed",
  footnotes: "Embed",
  toc: "Layout",
  callout: "Layout",
  timed: "Layout",
};

/** Non-native kinds: layout regions, or cards edited in the settings panel. */
const OTHER_ITEMS: Item[] = BLOCK_KINDS.filter((k) => !isNative(k.kind)).map(
  (k) => ({
    key: k.kind,
    label: k.label,
    hint: CONTAINER_KINDS.includes(k.kind)
      ? "A region you can write into"
      : "Edit its details in the panel",
    group: GROUP_OF[k.kind] ?? "Other",
    icon: k.icon,
    keywords: [k.kind, k.label.toLowerCase()],
    run: insertKind(k.kind),
  }),
);

const GROUP_ORDER = ["Text", "Media", "Layout", "Embed", "Assist", "Patterns", "Plugins", "Other"];

/**
 * Insert a saved pattern. Unsynced: its blocks are copied in and become
 * ordinary content. Synced: one reference block, resolved at render time,
 * so editing the pattern later changes this page too.
 */
function insertPattern(p: Pattern) {
  return (editor: Editor) => {
    if (p.synced) {
      const node = { type: "rpBlock", attrs: { kind: "pattern", attrs: { pattern_id: p.id, name: p.name }, children: [] } };
      editor.chain().focus().insertContent(node as unknown as Record<string, unknown>).run();
      return;
    }
    const nodes = (p.blocks as Block[]).map((b) => blockToNode(b));
    editor.chain().focus().insertContent(nodes as unknown as Record<string, unknown>[]).run();
  };
}

const ALL_ITEMS = [...TEXT_ITEMS, ...OTHER_ITEMS].sort(
  (a, b) => GROUP_ORDER.indexOf(a.group) - GROUP_ORDER.indexOf(b.group),
);

export interface SlashState {
  /** Where to anchor the menu, relative to the canvas wrapper. */
  top: number;
  left: number;
  /** Open downward from the caret, or upward when the caret is near the bottom. */
  placement: "below" | "above";
  /** Text typed after the "/". */
  query: string;
  /** Document position of the "/" so it can be removed on insert. */
  from: number;
}

export function SlashMenu({
  editor,
  state,
  onClose,
}: {
  editor: Editor;
  state: SlashState;
  onClose: () => void;
}) {
  const [active, setActive] = React.useState(0);
  const listRef = React.useRef<HTMLDivElement>(null);

  // Blocks contributed by enabled plugins. Fetched rather than hardcoded,
  // and folded in beside the core kinds so an author inserts one the same
  // way they insert a quote.
  const surface = useQuery({
    queryKey: ["plugin-surface"],
    queryFn: getPluginSurface,
    staleTime: 5 * 60 * 1000,
  });

  const patterns = useQuery({ queryKey: ["patterns"], queryFn: () => api.listPatterns(), staleTime: 60_000 });

  const ai = useAiAvailable();
  const query = state.query.toLowerCase();
  const all = React.useMemo(() => {
    const saved: Item[] = (patterns.data ?? []).map((p) => ({
      key: `pattern-${p.id}`,
      label: p.name,
      hint: p.synced ? "Synced: edits to the pattern show here too" : p.category || "Copied in as ordinary blocks",
      group: "Patterns",
      icon: p.synced ? "⟳" : "⧉",
      keywords: ["pattern", p.name.toLowerCase(), p.category.toLowerCase()],
      run: insertPattern(p),
    }));
    const plugin: Item[] = (surface.data?.blocks ?? []).map((b) => ({
      key: b.kind,
      label: b.title,
      hint: `From ${b.pluginName}`,
      group: "Plugins",
      icon: b.icon ?? "◇",
      keywords: [b.kind, b.title.toLowerCase(), b.pluginName.toLowerCase()],
      run: insertKind(b.kind as BlockKind),
    }));
    // An assistant that is not set up is not offered.
    const base = ai.text ? ALL_ITEMS : ALL_ITEMS.filter((i) => i.group !== "Assist");
    return [...base, ...saved, ...plugin];
  }, [surface.data, patterns.data, ai.text]);

  const items = React.useMemo(
    () =>
      query === ""
        ? all
        : all.filter(
            (i) =>
              i.label.toLowerCase().includes(query) ||
              i.keywords.some((k) => k.includes(query)),
          ),
    [query, all],
  );

  React.useEffect(() => setActive(0), [query]);

  const choose = React.useCallback(
    (item: Item) => {
      // Remove the "/query" the author typed, then run the command.
      editor
        .chain()
        .focus()
        .deleteRange({ from: state.from, to: state.from + state.query.length + 1 })
        .run();
      void item.run(editor);
      onClose();
    },
    [editor, state.from, state.query.length, onClose],
  );

  // Keyboard nav is captured before ProseMirror sees the key.
  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setActive((i) => (items.length === 0 ? 0 : (i + 1) % items.length));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setActive((i) =>
          items.length === 0 ? 0 : (i - 1 + items.length) % items.length,
        );
      } else if (e.key === "Enter" || e.key === "Tab") {
        const item = items[active];
        if (item !== undefined) {
          e.preventDefault();
          choose(item);
        }
      } else if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [items, active, choose, onClose]);

  React.useEffect(() => {
    listRef.current
      ?.querySelector('[data-active="true"]')
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const style: React.CSSProperties =
    state.placement === "below"
      ? { top: state.top, left: state.left }
      : { bottom: `calc(100% - ${state.top}px)`, left: state.left };

  if (items.length === 0) {
    return (
      <div
        style={style}
        className="absolute z-40 w-72 max-w-[calc(100vw-2rem)] rounded-lg border bg-popover p-3 text-sm text-muted-foreground shadow-lg"
      >
        No blocks match “{state.query}”.
      </div>
    );
  }

  let lastGroup = "";
  return (
    <div
      ref={listRef}
      role="listbox"
      aria-label="Insert a block"
      style={style}
      className="absolute z-40 max-h-72 w-72 max-w-[calc(100vw-2rem)] overflow-y-auto rounded-lg border bg-popover p-1.5 shadow-lg"
    >
      {items.map((item, index) => {
        const header = item.group !== lastGroup ? item.group : null;
        lastGroup = item.group;
        return (
          <React.Fragment key={item.key}>
            {header !== null ? (
              <p className="px-2 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                {header}
              </p>
            ) : null}
            <button
              type="button"
              role="option"
              aria-selected={index === active}
              data-active={index === active}
              onMouseEnter={() => setActive(index)}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => choose(item)}
              className={cn(
                "flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left",
                index === active ? "bg-accent" : "hover:bg-accent/60",
              )}
            >
              <span
                aria-hidden="true"
                className="flex h-6 w-6 shrink-0 items-center justify-center rounded border bg-background font-mono text-[10px] text-muted-foreground"
              >
                {item.icon}
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium">
                  {item.label}
                </span>
                <span className="block truncate text-xs text-muted-foreground">
                  {item.hint}
                </span>
              </span>
            </button>
          </React.Fragment>
        );
      })}
    </div>
  );
}
