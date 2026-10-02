import type { Editor } from "@tiptap/core";
import { useEditorState } from "@tiptap/react";
import {
  BetweenHorizonalStart,
  BetweenVerticalStart,
  Heading1,
  Rows3,
  Trash2,
  Columns3,
} from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * Table controls, shown beneath the canvas while the caret is inside a
 * table. Structure only — cell text is edited in place, Tab moves between
 * cells, and formatting comes from the bubble toolbar like anywhere else.
 */
export function TableToolbar({ editor }: { editor: Editor }) {
  const state = useEditorState({
    editor,
    selector: ({ editor: e }) => ({
      active: e.isActive("table"),
      caption: String(e.getAttributes("table")["caption"] ?? ""),
    }),
  });
  if (!state.active) return null;

  const run = (fn: (c: ReturnType<Editor["chain"]>) => ReturnType<Editor["chain"]>) => () =>
    fn(editor.chain().focus()).run();

  return (
    <div
      className="mt-2 flex flex-wrap items-center gap-1 rounded-lg border bg-card p-1.5"
      role="toolbar"
      aria-label="Table"
      data-testid="table-toolbar"
    >
      <Group label="Rows">
        <Btn label="Add row above" onClick={run((c) => c.addRowBefore())}>
          <BetweenHorizonalStart className="h-3.5 w-3.5 rotate-180" aria-hidden="true" />
        </Btn>
        <Btn label="Add row below" onClick={run((c) => c.addRowAfter())}>
          <BetweenHorizonalStart className="h-3.5 w-3.5" aria-hidden="true" />
        </Btn>
        <Btn label="Delete row" onClick={run((c) => c.deleteRow())}>
          <Rows3 className="h-3.5 w-3.5" aria-hidden="true" />
          <span className="sr-only">Delete row</span>
          <Trash2 className="h-3 w-3 text-destructive" aria-hidden="true" />
        </Btn>
      </Group>
      <Divider />
      <Group label="Columns">
        <Btn label="Add column before" onClick={run((c) => c.addColumnBefore())}>
          <BetweenVerticalStart className="h-3.5 w-3.5 rotate-180" aria-hidden="true" />
        </Btn>
        <Btn label="Add column after" onClick={run((c) => c.addColumnAfter())}>
          <BetweenVerticalStart className="h-3.5 w-3.5" aria-hidden="true" />
        </Btn>
        <Btn label="Delete column" onClick={run((c) => c.deleteColumn())}>
          <Columns3 className="h-3.5 w-3.5" aria-hidden="true" />
          <Trash2 className="h-3 w-3 text-destructive" aria-hidden="true" />
        </Btn>
      </Group>
      <Divider />
      <Btn label="Toggle header row" onClick={run((c) => c.toggleHeaderRow())}>
        <Heading1 className="h-3.5 w-3.5" aria-hidden="true" />
        Header row
      </Btn>
      <input
        value={state.caption}
        onChange={(e) => editor.chain().updateAttributes("table", { caption: e.target.value }).run()}
        placeholder="Caption (optional)"
        aria-label="Table caption"
        className="ml-1 h-7 min-w-0 flex-1 rounded-md border border-input bg-background px-2 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      />
      <Btn label="Delete table" destructive onClick={run((c) => c.deleteTable())}>
        <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
        Delete table
      </Btn>
    </div>
  );
}

function Group({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-0.5" role="group" aria-label={label}>
      {children}
    </div>
  );
}

function Divider() {
  return <span className="mx-0.5 h-4 w-px bg-border" aria-hidden="true" />;
}

function Btn({
  label,
  destructive,
  onClick,
  children,
}: {
  label: string;
  destructive?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onMouseDown={(e) => e.preventDefault()}
      onClick={onClick}
      className={cn(
        "inline-flex h-7 items-center gap-1 rounded-md px-1.5 text-xs transition-colors",
        destructive === true
          ? "text-muted-foreground hover:bg-destructive-subtle hover:text-destructive"
          : "text-muted-foreground hover:bg-accent hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}
