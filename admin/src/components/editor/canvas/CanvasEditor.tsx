import * as React from "react";
import { EditorContent, useEditor } from "@tiptap/react";
import type { Editor } from "@tiptap/core";
import { NodeSelection } from "@tiptap/pm/state";
import { buildExtensions } from "./extensions";
import { cleanPastedHtml } from "../paste";
import { looksLikeMarkdown, markdownToHtml } from "../markdownPaste";
import {
  blocksToDoc,
  docToBlocks,
  isEmptyDoc,
  type PMNode,
} from "./serialize";
import { BubbleToolbar } from "./BubbleToolbar";
import { SlashMenu, type SlashState } from "./SlashMenu";
import { OpaqueBlockPanel } from "./OpaqueBlockPanel";
import { DragHandle } from "./DragHandle";
import { TableToolbar } from "./TableToolbar";
import { MobileToolbar } from "./MobileToolbar";
import { imageFilesFrom, insertImages } from "./upload";
import type { Block } from "../blocks";

export interface CanvasHandle {
  /** Put the caret at the start of the document. */
  focus: () => void;
}

/**
 * The writing canvas.
 *
 * Blocks render as their output rather than as labelled form fields. Content
 * stays `BlockDocument v1` on the wire — see ./serialize.ts, which guarantees
 * blocks the canvas cannot edit natively survive a round trip untouched.
 */
export const CanvasEditor = React.forwardRef<
  CanvasHandle,
  {
    value: Block[];
    onChange: (blocks: Block[]) => void;
  }
>(function CanvasEditor({ value, onChange }, ref) {
  const wrapRef = React.useRef<HTMLDivElement>(null);
  const editorRef = React.useRef<Editor | null>(null);
  const [slash, setSlash] = React.useState<SlashState | null>(null);
  const [selectedOpaque, setSelectedOpaque] = React.useState<number | null>(null);

  // What we last emitted upward. The parent stores it and hands it straight
  // back, and re-seeding the editor from our own echo would fight the caret.
  const lastEmitted = React.useRef<Block[] | null>(null);

  const editor = useEditor({
    extensions: buildExtensions(),
    content: blocksToDoc(value) as unknown as Record<string, unknown>,
    editorProps: {
      attributes: {
        class: "vy-canvas prose-none focus:outline-none min-h-[50vh] px-1 py-2",
        "aria-label": "Post content",
      },
      // Rich pastes (Google Docs, Word, Notion) are normalised first:
      // styled spans become real strong/em marks and the bold-in-name-
      // only clipboard wrapper is unwrapped. See ./paste.ts.
      transformPastedHTML: cleanPastedHtml,
      // Pasted or dropped image files become image blocks and upload
      // themselves; anything else is left to ProseMirror.
      handlePaste: (_view, event) => {
        const files = imageFilesFrom(event.clipboardData);
        const e = editorRef.current;
        if (e === null) return false;
        if (files.length > 0) {
          event.preventDefault();
          insertImages(e, files);
          return true;
        }
        // Plain text that reads as Markdown becomes real blocks. HTML
        // pastes carry their own structure and are left to the parser.
        const html = event.clipboardData?.getData("text/html") ?? "";
        const text = event.clipboardData?.getData("text/plain") ?? "";
        if (html.trim() === "" && looksLikeMarkdown(text) && !e.isActive("codeBlock")) {
          event.preventDefault();
          e.chain().focus().insertContent(markdownToHtml(text)).run();
          return true;
        }
        return false;
      },
      handleDrop: (view, event, _slice, moved) => {
        if (moved) return false;
        const files = imageFilesFrom(event.dataTransfer);
        const e = editorRef.current;
        if (files.length === 0 || e === null) return false;
        event.preventDefault();
        const at = view.posAtCoords({ left: event.clientX, top: event.clientY })?.pos;
        insertImages(e, files, at);
        return true;
      },
    },
    onUpdate: ({ editor: e }) => {
      const json = e.getJSON() as unknown as PMNode;
      const blocks = isEmptyDoc(json) ? [] : docToBlocks(json);
      lastEmitted.current = blocks;
      onChange(blocks);
    },
    onSelectionUpdate: ({ editor: e }) => {
      const { selection } = e.state;
      setSelectedOpaque(
        selection instanceof NodeSelection && selection.node.type.name === "rpBlock"
          ? selection.from
          : null,
      );
    },
  });
  editorRef.current = editor;

  React.useImperativeHandle(
    ref,
    () => ({
      focus: () => {
        editor?.commands.focus("start");
      },
    }),
    [editor],
  );

  // Re-seed only when the value genuinely came from elsewhere (a revision
  // restore, or the JSON pane), never from our own onUpdate.
  React.useEffect(() => {
    if (editor === null) return;
    if (lastEmitted.current === value) return;
    const next = blocksToDoc(value);
    const current = editor.getJSON() as unknown as PMNode;
    if (JSON.stringify(current) === JSON.stringify(next)) return;
    editor.commands.setContent(next as unknown as Record<string, unknown>, {
      emitUpdate: false,
    });
  }, [editor, value]);

  // "/" at the start of an empty text block opens the inserter. The menu is
  // positioned relative to the wrapper so it scrolls with the text, and it
  // opens upward when the caret is near the bottom of the viewport.
  React.useEffect(() => {
    if (editor === null) return;

    const update = () => {
      const { state } = editor;
      const { $from, empty } = state.selection;
      if (!empty || editor.isActive("codeBlock") || !$from.parent.isTextblock) {
        setSlash(null);
        return;
      }
      const textBefore = $from.parent.textBetween(
        0,
        $from.parentOffset,
        undefined,
        "￼",
      );
      const match = /(?:^|\s)\/([\w-]*)$/.exec(textBefore);
      if (match === null || match[1] === undefined) {
        setSlash(null);
        return;
      }
      const query = match[1];
      const from = $from.pos - query.length - 1;
      const caret = editor.view.coordsAtPos(from);
      const wrap = wrapRef.current?.getBoundingClientRect();
      if (wrap === undefined) return;
      const placement =
        window.innerHeight - caret.bottom < 320 && caret.top > 320 ? "above" : "below";
      setSlash({
        query,
        from,
        placement,
        top: (placement === "below" ? caret.bottom + 6 : caret.top - 6) - wrap.top,
        left: Math.max(0, caret.left - wrap.left),
      });
    };

    editor.on("transaction", update);
    return () => {
      editor.off("transaction", update);
    };
  }, [editor]);

  if (editor === null) {
    return (
      <div className="min-h-[50vh] animate-pulse rounded-lg bg-muted/40" />
    );
  }

  return (
    <div ref={wrapRef} className="vy-canvas-wrap relative" data-testid="canvas-editor">
      <BubbleToolbar editor={editor} />
      <DragHandle editor={editor} container={wrapRef} />
      <EditorContent editor={editor} />
      {slash !== null ? (
        <SlashMenu
          editor={editor}
          state={slash}
          onClose={() => setSlash(null)}
        />
      ) : null}
      <TableToolbar editor={editor} />
      {selectedOpaque !== null ? (
        <OpaqueBlockPanel editor={editor} pos={selectedOpaque} />
      ) : null}
      <MobileToolbar editor={editor} />
      <CanvasStyles />
    </div>
  );
});

/** Editor-only typography. Scoped to `.vy-canvas` so it cannot leak. */
function CanvasStyles() {
  return (
    <style>{`
      /* The measure is the writer's choice (Measure / Wide / Full in the
         toolbar); PostEditor sets --vy-canvas-measure on the column. */
      .vy-canvas-wrap { max-width: var(--vy-canvas-measure, 70ch); margin-inline: auto; }
      /* Desktop: a gutter on the left for the block handle, so it never sits
         on top of the previous line. The title gets the same offset. */
      @media (min-width: 1024px) {
        .vy-canvas-wrap { max-width: calc(var(--vy-canvas-measure, 70ch) + 7.5rem); padding-left: 7.5rem; }
      }
      /* The global keyboard focus ring is right for buttons and wrong for a
         page of prose: the caret is the focus indicator here. */
      .vy-canvas:focus-visible, .vy-canvas:focus { outline: none; box-shadow: none; }
      .vy-canvas > * + * { margin-top: 0.75rem; }
      .vy-canvas p { line-height: 1.75; }
      .vy-canvas h2 { font-size: 1.5rem; font-weight: 650; letter-spacing: -0.02em; margin-top: 1.75rem; line-height: 1.25; }
      .vy-canvas h3 { font-size: 1.25rem; font-weight: 640; letter-spacing: -0.015em; margin-top: 1.5rem; line-height: 1.3; }
      .vy-canvas h4, .vy-canvas h5, .vy-canvas h6 { font-size: 1.05rem; font-weight: 640; margin-top: 1.25rem; }
      .vy-canvas ul, .vy-canvas ol { padding-left: 1.5rem; }
      .vy-canvas ul { list-style: disc; }
      .vy-canvas ol { list-style: decimal; }
      .vy-canvas ul ul { list-style: circle; }
      .vy-canvas li { margin: 0.2rem 0; }
      .vy-canvas li > p { margin: 0; }
      .vy-canvas li > ul, .vy-canvas li > ol { margin-top: 0.2rem; }
      .vy-canvas blockquote {
        border-left: 3px solid hsl(var(--primary));
        padding-left: 1rem;
        color: hsl(var(--muted-foreground));
        font-style: italic;
      }
      .vy-canvas blockquote[data-citation]:not([data-citation=""])::after {
        content: "— " attr(data-citation);
        display: block;
        margin-top: 0.35rem;
        font-size: 0.8125rem;
        font-style: normal;
      }
      .vy-canvas pre {
        background: hsl(var(--muted));
        border: 1px solid hsl(var(--border));
        border-radius: 0.5rem;
        padding: 0.75rem 1rem;
        overflow-x: auto;
        font-family: ui-monospace, "SF Mono", Menlo, monospace;
        font-size: 0.8125rem;
        line-height: 1.6;
        tab-size: 2;
      }
      .vy-codeblock pre { padding-right: 7rem; }
      .vy-canvas code { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 0.875em; }
      .vy-canvas :not(pre) > code {
        background: hsl(var(--muted));
        border: 1px solid hsl(var(--border));
        border-radius: 0.25rem;
        padding: 0.05em 0.3em;
      }
      .vy-canvas mark { background: color-mix(in oklab, hsl(var(--primary)) 22%, transparent); color: inherit; border-radius: 2px; padding: 0 0.1em; }
      .vy-canvas s { text-decoration-color: hsl(var(--muted-foreground)); }
      .vy-canvas a { color: hsl(var(--primary)); text-decoration: underline; text-underline-offset: 2px; }
      .vy-canvas hr { border: none; border-top: 1px solid hsl(var(--border)); margin: 1.5rem 0; }

      /* Syntax colours mixed with the foreground so they read in both themes. */
      .vy-canvas .hljs-comment, .vy-canvas .hljs-quote { color: hsl(var(--muted-foreground)); font-style: italic; }
      .vy-canvas .hljs-keyword, .vy-canvas .hljs-selector-tag, .vy-canvas .hljs-literal, .vy-canvas .hljs-type, .vy-canvas .hljs-meta { color: hsl(var(--primary)); }
      .vy-canvas .hljs-string, .vy-canvas .hljs-regexp, .vy-canvas .hljs-addition { color: color-mix(in oklab, #16a34a 75%, hsl(var(--foreground))); }
      .vy-canvas .hljs-number, .vy-canvas .hljs-symbol, .vy-canvas .hljs-bullet, .vy-canvas .hljs-link { color: color-mix(in oklab, #d97706 75%, hsl(var(--foreground))); }
      .vy-canvas .hljs-title, .vy-canvas .hljs-function, .vy-canvas .hljs-name, .vy-canvas .hljs-section { color: color-mix(in oklab, #7c3aed 70%, hsl(var(--foreground))); }
      .vy-canvas .hljs-attr, .vy-canvas .hljs-attribute, .vy-canvas .hljs-variable, .vy-canvas .hljs-template-variable, .vy-canvas .hljs-property { color: color-mix(in oklab, #0891b2 70%, hsl(var(--foreground))); }
      .vy-canvas .hljs-built_in, .vy-canvas .hljs-class { color: color-mix(in oklab, #db2777 65%, hsl(var(--foreground))); }
      .vy-canvas .hljs-deletion { color: hsl(var(--destructive)); }
      .vy-canvas .hljs-emphasis { font-style: italic; }
      .vy-canvas .hljs-strong { font-weight: 600; }

      /* Images */
      .vy-canvas figure.vy-image { margin: 1.25rem 0; }
      .vy-canvas figure.vy-image img { display: block; max-width: 100%; height: auto; border-radius: 0.5rem; margin-inline: auto; }
      .vy-canvas figure.vy-image.is-selected img { outline: 2px solid hsl(var(--primary)); outline-offset: 2px; }
      .vy-canvas .vy-image-caption { margin-top: 0.5rem; text-align: center; font-size: 0.875rem; color: hsl(var(--muted-foreground)); min-height: 1.25rem; }
      .vy-canvas figure.vy-image.is-empty .vy-image-caption::before {
        content: attr(data-placeholder);
        color: hsl(var(--muted-foreground) / 0.7);
        pointer-events: none;
        float: left;
        width: 100%;
        height: 0;
      }
      .vy-canvas .vy-image-empty {
        border: 1px dashed hsl(var(--border));
        border-radius: 0.75rem;
        padding: 1.5rem 1rem;
        text-align: center;
        background: hsl(var(--muted) / 0.35);
      }
      .vy-canvas .vy-image-tools {
        position: absolute; top: 0.5rem; left: 50%; transform: translateX(-50%);
        display: flex; gap: 0.125rem; align-items: center;
        background: hsl(var(--popover)); border: 1px solid hsl(var(--border));
        border-radius: 0.5rem; padding: 0.125rem; box-shadow: 0 4px 12px rgb(0 0 0 / 0.12);
        white-space: nowrap;
      }
      .vy-canvas .vy-image-panel { margin: 0.5rem auto 0; max-width: 32rem; border: 1px solid hsl(var(--border)); border-radius: 0.5rem; background: hsl(var(--card)); padding: 0.625rem; }
      /* On a phone the floating strip is wider than the picture; it sits
         under the image instead, wrapping, with thumb-sized buttons. */
      @media (max-width: 640px) {
        .vy-canvas .vy-image-tools {
          position: static; transform: none; margin-top: 0.5rem;
          flex-wrap: wrap; justify-content: center; white-space: normal; box-shadow: none;
        }
        .vy-canvas .vy-image-tools button, .vy-canvas .vy-image-tools a { min-height: 40px; }
      }

      /* Tables */
      .vy-canvas table { border-collapse: collapse; width: 100%; margin: 1rem 0; table-layout: fixed; font-size: 0.9375rem; }
      .vy-canvas td, .vy-canvas th { border: 1px solid hsl(var(--border)); padding: 0.4rem 0.6rem; vertical-align: top; position: relative; min-width: 3rem; }
      .vy-canvas th { background: hsl(var(--muted) / 0.6); font-weight: 600; text-align: left; }
      .vy-canvas td > p, .vy-canvas th > p { margin: 0; line-height: 1.5; }
      .vy-canvas .selectedCell::after { content: ""; position: absolute; inset: 0; background: hsl(var(--primary) / 0.12); pointer-events: none; }

      /* Containers */
      .vy-container {
        border: 1px dashed hsl(var(--border));
        border-radius: 0.5rem;
        padding: 0.25rem 0.75rem 0.75rem;
        margin: 0.75rem 0;
        background: hsl(var(--muted) / 0.35);
      }
      .vy-container-label {
        font-size: 0.625rem;
        font-weight: 600;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        color: hsl(var(--muted-foreground));
        padding: 0.25rem 0;
        user-select: none;
      }
      .vy-container[data-vy-container="columns"] > .vy-container-content,
      .vy-container[data-vy-container="row"] > .vy-container-content {
        display: grid;
        gap: 0.75rem;
        grid-template-columns: repeat(2, minmax(0, 1fr));
      }
      .vy-container[data-vy-container="grid"] > .vy-container-content {
        display: grid;
        gap: 0.75rem;
        grid-template-columns: repeat(auto-fit, minmax(11rem, 1fr));
      }
      .vy-container[data-vy-container="buttons"] > .vy-container-content {
        display: flex;
        flex-wrap: wrap;
        gap: 0.5rem;
      }
      .vy-container-content > * + * { margin-top: 0.5rem; }
      .vy-container[data-vy-container="callout"] { border-style: solid; border-left-width: 4px; border-left-color: hsl(var(--primary)); }
      .vy-container[data-vy-tone="tip"] { border-left-color: #2e8b57; }
      .vy-container[data-vy-tone="warning"] { border-left-color: #c8891a; }
      .vy-container[data-vy-tone="danger"] { border-left-color: #c0392b; }
      .vy-container[data-vy-container="timed"] { border-color: hsl(var(--primary) / 0.5); }
      .vy-container[data-vy-container="columns"] > .vy-container-content > * + *,
      .vy-container[data-vy-container="row"] > .vy-container-content > * + *,
      .vy-container[data-vy-container="grid"] > .vy-container-content > * + * {
        margin-top: 0;
      }
      @media (max-width: 640px) {
        .vy-container > .vy-container-content { grid-template-columns: 1fr !important; }
      }
      .vy-canvas .ProseMirror-selectednode { outline: 2px solid hsl(var(--primary)); outline-offset: 2px; border-radius: 0.5rem; }
      .vy-canvas .ProseMirror-gapcursor::after { border-top-color: hsl(var(--foreground)); }
      .vy-canvas p.is-editor-empty:first-child::before,
      .vy-canvas h2.is-empty::before, .vy-canvas h3.is-empty::before {
        content: attr(data-placeholder);
        color: hsl(var(--muted-foreground));
        float: left;
        height: 0;
        pointer-events: none;
      }
    `}</style>
  );
}

export type { Editor };
