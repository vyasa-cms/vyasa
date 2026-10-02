import * as React from "react";
import { createPortal } from "react-dom";
import type { Editor } from "@tiptap/core";
import { useEditorState } from "@tiptap/react";
import {
  Bold,
  ChevronDown,
  ChevronUp,
  Copy,
  Heading2,
  Heading3,
  Image as ImageIcon,
  Italic,
  Link2,
  List,
  ListOrdered,
  Quote,
  Redo2,
  Trash2,
  Underline,
  Undo2,
} from "lucide-react";
import { promptLink } from "./extensions";
import { currentBlock, duplicateBlock, moveBlock, removeBlock } from "./blockOps";
import { MOBILE_TOOLBAR_SLOT } from "./slot";
import { insertImages } from "./upload";
import { cn } from "@/lib/utils";

/**
 * A fixed formatting strip for phones. Bubble menus fight the native
 * selection handles on touch, and there is no hover for the drag handle, so
 * the common actions — formatting, headings, lists, links, images, undo,
 * and moving the current block — live in one always-reachable row.
 *
 * Rendered into a slot owned by the post editor so it stacks correctly
 * with the save bar rather than guessing at its height.
 */
export function MobileToolbar({ editor }: { editor: Editor }) {
  const [slot, setSlot] = React.useState<HTMLElement | null>(null);
  const fileRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    setSlot(document.getElementById(MOBILE_TOOLBAR_SLOT));
  }, []);

  const state = useEditorState({
    editor,
    selector: ({ editor: e }) => {
      const block = currentBlock(e);
      return {
        bold: e.isActive("bold"),
        italic: e.isActive("italic"),
        underline: e.isActive("underline"),
        link: e.isActive("link"),
        h2: e.isActive("heading", { level: 2 }),
        h3: e.isActive("heading", { level: 3 }),
        bullet: e.isActive("bulletList"),
        ordered: e.isActive("orderedList"),
        quote: e.isActive("blockquote"),
        canUndo: e.can().undo(),
        canRedo: e.can().redo(),
        inCode: e.isActive("codeBlock"),
        blockIndex: block?.index ?? 0,
        blockCount: block?.count ?? 0,
      };
    },
  });

  if (slot === null) return null;

  const chain = () => editor.chain().focus();
  const withBlock = (fn: (b: NonNullable<ReturnType<typeof currentBlock>>) => void) => () => {
    const block = currentBlock(editor);
    if (block !== null) fn(block);
  };

  return createPortal(
    <div
      className="flex items-center gap-0.5 overflow-x-auto px-2 py-1.5"
      role="toolbar"
      aria-label="Formatting"
      data-testid="mobile-toolbar"
    >
      <Btn label="Undo" disabled={!state.canUndo} onClick={() => chain().undo().run()}>
        <Undo2 className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Redo" disabled={!state.canRedo} onClick={() => chain().redo().run()}>
        <Redo2 className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Sep />
      <Btn label="Bold" active={state.bold} disabled={state.inCode} onClick={() => chain().toggleBold().run()}>
        <Bold className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Italic" active={state.italic} disabled={state.inCode} onClick={() => chain().toggleItalic().run()}>
        <Italic className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Underline" active={state.underline} disabled={state.inCode} onClick={() => chain().toggleUnderline().run()}>
        <Underline className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Link" active={state.link} disabled={state.inCode} onClick={() => promptLink(editor)}>
        <Link2 className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Sep />
      <Btn label="Heading 2" active={state.h2} onClick={() => chain().toggleHeading({ level: 2 }).run()}>
        <Heading2 className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Heading 3" active={state.h3} onClick={() => chain().toggleHeading({ level: 3 }).run()}>
        <Heading3 className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Bulleted list" active={state.bullet} onClick={() => chain().toggleBulletList().run()}>
        <List className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Numbered list" active={state.ordered} onClick={() => chain().toggleOrderedList().run()}>
        <ListOrdered className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Quote" active={state.quote} onClick={() => chain().toggleBlockquote().run()}>
        <Quote className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Add image" onClick={() => fileRef.current?.click()}>
        <ImageIcon className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Sep />
      <Btn label="Move block up" disabled={state.blockIndex === 0} onClick={withBlock((b) => moveBlock(editor, b, -1))}>
        <ChevronUp className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn
        label="Move block down"
        disabled={state.blockIndex >= state.blockCount - 1}
        onClick={withBlock((b) => moveBlock(editor, b, 1))}
      >
        <ChevronDown className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Duplicate block" onClick={withBlock((b) => duplicateBlock(editor, b))}>
        <Copy className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <Btn label="Delete block" destructive onClick={withBlock((b) => removeBlock(editor, b))}>
        <Trash2 className="h-4 w-4" aria-hidden="true" />
      </Btn>
      <input
        ref={fileRef}
        type="file"
        accept="image/*,.heic,.heif"
        multiple
        hidden
        onChange={(e) => {
          const files = Array.from(e.target.files ?? []);
          e.target.value = "";
          if (files.length > 0) insertImages(editor, files);
        }}
      />
    </div>,
    slot,
  );
}

function Sep() {
  return <span className="mx-1 h-5 w-px shrink-0 bg-border" aria-hidden="true" />;
}

function Btn({
  label,
  active,
  disabled,
  destructive,
  onClick,
  children,
}: {
  label: string;
  active?: boolean;
  disabled?: boolean;
  destructive?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      aria-pressed={active}
      title={label}
      disabled={disabled}
      // Pointer-down would move focus (and the selection) out of the editor.
      onPointerDown={(e) => e.preventDefault()}
      onClick={onClick}
      className={cn(
        "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md transition-colors disabled:opacity-30",
        active === true
          ? "bg-primary text-primary-foreground"
          : destructive === true
            ? "text-muted-foreground hover:text-destructive"
            : "text-muted-foreground hover:bg-accent hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}
