import * as React from "react";
import type { Editor } from "@tiptap/core";
import { ChevronDown, ChevronUp, Copy, GripVertical, Bookmark, Trash2 } from "lucide-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/client";
import { nodeToBlock } from "./serialize";
import { Modal } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import {
  currentBlock,
  duplicateBlock,
  locateBlock,
  moveBlock,
  removeBlock,
  selectBlock,
  type BlockTarget,
} from "./blockOps";
import { useEditorDom } from "./useEditorDom";
import { cn } from "@/lib/utils";

interface Placed {
  block: BlockTarget;
  /** Relative to the canvas wrapper, so it scrolls with the content. */
  top: number;
  left: number;
}

/**
 * Floating block controls: drag to reorder, plus keyboard-reachable move,
 * duplicate and delete.
 *
 * The handle follows the block the caret is in, and the block under the
 * pointer while hovering, so it is there whether an author reaches for the
 * mouse or not. Blocks inside editable containers get the same controls as
 * top-level ones. Phones use the fixed toolbar instead — there is no hover,
 * and no room to the left of the text.
 */
export function DragHandle({
  editor,
  container,
}: {
  editor: Editor;
  container: React.RefObject<HTMLDivElement | null>;
}) {
  const [placed, setPlaced] = React.useState<Placed | null>(null);
  const hover = React.useRef<BlockTarget | null>(null);
  // "Save as pattern": the hovered block, serialised, named, and kept.
  const [saving, setSaving] = React.useState<BlockTarget | null>(null);
  const [patternName, setPatternName] = React.useState("");
  const [synced, setSynced] = React.useState(false);
  const queryClient = useQueryClient();
  const savePattern = useMutation({
    mutationFn: () => {
      const block = saving ? nodeToBlock(saving.node.toJSON() as unknown as Parameters<typeof nodeToBlock>[0]) : null;
      if (!block) return Promise.reject(new Error("this block cannot be saved"));
      return api.createPattern({ name: patternName.trim(), category: "", synced, blocks: [block] });
    },
    onSuccess: (p) => {
      setSaving(null);
      setPatternName("");
      setSynced(false);
      void queryClient.invalidateQueries({ queryKey: ["patterns"] });
      notify.success(`Saved as "${p.name}"`, p.synced ? "Insert it anywhere with / and edit it once for every page." : "Insert it anywhere with /.");
    },
    onError: (e) => notify.error("Couldn't save the pattern", e),
  });

  const place = React.useCallback(
    (block: BlockTarget | null) => {
      const wrap = container.current;
      if (block === null || wrap === null) {
        setPlaced(null);
        return;
      }
      const node = ((): Node | null => {
        try {
          return editor.view.nodeDOM(block.pos);
        } catch {
          return null; // view not mounted yet
        }
      })();
      const el = node instanceof HTMLElement ? node : (node?.parentElement ?? null);
      if (el === null) {
        setPlaced(null);
        return;
      }
      const rect = el.getBoundingClientRect();
      const base = wrap.getBoundingClientRect();
      setPlaced({
        block,
        top: rect.top - base.top,
        left: rect.left - base.left,
      });
    },
    [container, editor],
  );

  const dom = useEditorDom(editor);
  React.useEffect(() => {
    if (dom === null) return;

    const onMove = (event: MouseEvent) => {
      const found = editor.view.posAtCoords({ left: event.clientX, top: event.clientY });
      if (found === null) return;
      // Hovering a container's label targets the container itself.
      const label = (event.target as HTMLElement | null)?.closest?.(".vy-container-label");
      const pos =
        label !== null && label !== undefined && found.inside >= 0 ? found.inside : found.pos;
      const block = locateBlock(editor, pos);
      hover.current = block;
      place(block);
    };
    const onLeave = (event: MouseEvent) => {
      const to = event.relatedTarget;
      if (to instanceof HTMLElement && to.closest("[data-drag-handle-ui]")) return;
      hover.current = null;
      place(editor.isFocused ? currentBlock(editor) : null);
    };
    const onTransaction = () => {
      if (hover.current === null) place(editor.isFocused ? currentBlock(editor) : null);
    };
    const onBlur = () => {
      if (hover.current === null) setPlaced(null);
    };

    dom.addEventListener("mousemove", onMove);
    dom.addEventListener("mouseleave", onLeave);
    editor.on("transaction", onTransaction);
    editor.on("blur", onBlur);
    editor.on("focus", onTransaction);
    return () => {
      dom.removeEventListener("mousemove", onMove);
      dom.removeEventListener("mouseleave", onLeave);
      editor.off("transaction", onTransaction);
      editor.off("blur", onBlur);
      editor.off("focus", onTransaction);
    };
  }, [editor, dom, place]);

  if (placed === null) return null;

  const { block } = placed;
  // 84px of controls in the gutter to the left of the block (the canvas
  // reserves one on desktop); if a block sits flush left anyway, hang the
  // controls off its top-right corner rather than over the previous line.
  const fitsLeft = placed.left >= 116;
  const style: React.CSSProperties = fitsLeft
    ? { top: placed.top, left: placed.left - 10, transform: "translateX(-100%)" }
    : { top: placed.top, right: 0, transform: "translateY(-50%)" };

  const act = (fn: () => void) => () => {
    fn();
    hover.current = null;
  };

  return (
    <div
      data-drag-handle-ui=""
      style={style}
      className="absolute z-30 hidden items-center gap-0.5 rounded-md border bg-popover p-0.5 shadow-xs lg:flex"
      onMouseLeave={() => {
        hover.current = null;
        place(editor.isFocused ? currentBlock(editor) : null);
      }}
    >
      <HandleButton
        label="Move block up"
        disabled={block.index === 0}
        onClick={act(() => moveBlock(editor, block, -1))}
      >
        <ChevronUp className="h-3.5 w-3.5" aria-hidden="true" />
      </HandleButton>
      <HandleButton
        label="Move block down"
        disabled={block.index >= block.count - 1}
        onClick={act(() => moveBlock(editor, block, 1))}
      >
        <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" />
      </HandleButton>

      {/* ProseMirror handles the drop itself once the node is selected. */}
      <button
        type="button"
        draggable
        aria-label="Drag to reorder this block"
        title="Drag to reorder"
        onMouseDown={() => selectBlock(editor, block)}
        onDragStart={() => selectBlock(editor, block)}
        className="inline-flex h-6 w-5 cursor-grab items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground active:cursor-grabbing"
      >
        <GripVertical className="h-3.5 w-3.5" aria-hidden="true" />
      </button>

      <HandleButton label="Duplicate block" onClick={act(() => duplicateBlock(editor, block))}>
        <Copy className="h-3.5 w-3.5" aria-hidden="true" />
      </HandleButton>
      <HandleButton label="Save as pattern" onClick={act(() => setSaving(block))}>
        <Bookmark className="h-3.5 w-3.5" aria-hidden="true" />
      </HandleButton>
      {saving ? (
        <Modal
          open
          onClose={() => setSaving(null)}
          title="Save as a pattern"
          description="Reuse this block anywhere from the / menu."
          testId="save-pattern"
          footer={<><Button variant="outline" onClick={() => setSaving(null)}>Cancel</Button><Button disabled={patternName.trim() === "" || savePattern.isPending} onClick={() => savePattern.mutate()}>{savePattern.isPending ? "Saving…" : "Save pattern"}</Button></>}
        >
          <div className="space-y-3">
            <Field label="Name" htmlFor="pattern-name"><Input id="pattern-name" autoFocus value={patternName} onChange={(e) => setPatternName(e.target.value)} placeholder="Call to action" /></Field>
            <label className="flex items-start gap-2 text-sm">
              <input type="checkbox" className="mt-1 accent-primary" checked={synced} onChange={(e) => setSynced(e.target.checked)} />
              <span><b>Synced</b><br /><span className="text-muted-foreground">Pages show the pattern as it is now; edit it once under Patterns and every page follows. Unsynced patterns are copied in and then edited freely.</span></span>
            </label>
          </div>
        </Modal>
      ) : null}
      <HandleButton label="Delete block" destructive onClick={act(() => removeBlock(editor, block))}>
        <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
      </HandleButton>
    </div>
  );
}

function HandleButton({
  label,
  onClick,
  disabled,
  destructive,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  destructive?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      title={label}
      className={cn(
        "inline-flex h-6 w-5 items-center justify-center rounded text-muted-foreground transition-colors disabled:opacity-30",
        destructive === true
          ? "hover:bg-destructive-subtle hover:text-destructive"
          : "hover:bg-accent hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}
