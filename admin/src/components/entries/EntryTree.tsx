import * as React from "react";

import { indent, moveDown, moveUp, movesForDrop, outdent, placementAt, rows, type Move, type Placement, type TreeNode } from "@/lib/tree";

/** A node as the entries page shows it. */
export interface EntryNode extends TreeNode {
  status: string;
}

const button = "rounded border px-1.5 py-0.5 text-xs hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40";

/**
 * The tree view of a hierarchical type's entries.
 *
 * Presentational: it draws `nodes`, and every way of moving an entry (drop,
 * the four buttons, alt+arrows) ends in `onMoves` with exactly the body of
 * `PUT /post-types/{type}/tree`. The page owns fetching and the mutation.
 */
export function EntryTree({
  nodes,
  busy,
  onMoves,
  href,
}: {
  nodes: EntryNode[];
  busy?: boolean;
  onMoves: (moves: Move[]) => void;
  href: (id: string) => string;
}) {
  const [dragging, setDragging] = React.useState<string | null>(null);
  const [over, setOver] = React.useState<{ id: string; placement: Placement } | null>(null);
  const list = React.useMemo(() => rows(nodes), [nodes]);

  const run = (moves: Move[]) => {
    if (moves.length > 0 && busy !== true) onMoves(moves);
  };

  const onDragOver = (event: React.DragEvent<HTMLLIElement>, id: string) => {
    if (dragging === null) return;
    event.preventDefault();
    const rect = event.currentTarget.getBoundingClientRect();
    const ratio = rect.height === 0 ? 0.5 : (event.clientY - rect.top) / rect.height;
    setOver({ id, placement: placementAt(ratio) });
  };

  return (
    <ul role="tree" aria-label="Entries" className="space-y-1">
      {list.map(({ node, depth }) => {
        const index = list.findIndex((r) => r.node.id === node.id);
        const up = moveUp(nodes, node.id);
        const down = moveDown(nodes, node.id);
        const inn = indent(nodes, node.id);
        const out = outdent(nodes, node.id);
        const marker = over?.id === node.id ? over.placement : null;
        return (
          <li
            key={node.id}
            role="treeitem"
            aria-level={depth}
            aria-selected={false}
            aria-posinset={index + 1}
            aria-setsize={list.length}
            draggable
            data-drop={marker ?? undefined}
            className={`flex items-center gap-2 rounded border bg-card px-2 py-1 ${marker === "before" ? "border-t-2 border-t-primary" : ""} ${marker === "after" ? "border-b-2 border-b-primary" : ""} ${marker === "inside" ? "ring-2 ring-primary" : ""}`}
            style={{ marginLeft: `${(depth - 1) * 1.5}rem` }}
            onDragStart={(e) => {
              setDragging(node.id);
              e.dataTransfer.setData("text/plain", node.id);
              e.dataTransfer.effectAllowed = "move";
            }}
            onDragOver={(e) => onDragOver(e, node.id)}
            onDragLeave={() => setOver((o) => (o?.id === node.id ? null : o))}
            onDragEnd={() => {
              setDragging(null);
              setOver(null);
            }}
            onDrop={(e) => {
              e.preventDefault();
              if (dragging !== null && over !== null && over.id === node.id) run(movesForDrop(nodes, dragging, node.id, over.placement));
              setDragging(null);
              setOver(null);
            }}
            onKeyDown={(e) => {
              if (!e.altKey) return;
              const pick = e.key === "ArrowUp" ? up : e.key === "ArrowDown" ? down : e.key === "ArrowRight" ? inn : e.key === "ArrowLeft" ? out : null;
              if (pick === null) return;
              e.preventDefault();
              run(pick);
            }}
            tabIndex={0}
          >
            <span aria-hidden="true" className="cursor-grab select-none text-muted-foreground">
              ⋮⋮
            </span>
            <a href={href(node.id)} className="flex-1 truncate text-sm font-medium hover:underline">
              {node.title || "(untitled)"}
            </a>
            {node.status === "published" ? null : <span className="rounded bg-muted px-1.5 text-xs">{node.status}</span>}
            <button type="button" className={button} aria-label={`Move ${node.title} up`} disabled={up.length === 0 || busy === true} onClick={() => run(up)}>
              Up
            </button>
            <button type="button" className={button} aria-label={`Move ${node.title} down`} disabled={down.length === 0 || busy === true} onClick={() => run(down)}>
              Down
            </button>
            <button type="button" className={button} aria-label={`Indent ${node.title}`} disabled={inn.length === 0 || busy === true} onClick={() => run(inn)}>
              Indent
            </button>
            <button type="button" className={button} aria-label={`Outdent ${node.title}`} disabled={out.length === 0 || busy === true} onClick={() => run(out)}>
              Outdent
            </button>
          </li>
        );
      })}
    </ul>
  );
}
