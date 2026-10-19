/**
 * Tree editing for hierarchical content types, as pure functions.
 *
 * The entries page draws `rows(nodes)` and, on a drop or a keyboard move,
 * asks one of the functions below which entries change. What comes back is
 * exactly the body of `PUT /api/v1/post-types/{type}/tree`: only entries
 * whose parent or position actually changes, with the new sibling list
 * renumbered in steps of ten so later inserts do not force a renumber.
 */

export interface TreeNode {
  /** A string: ids are snowflakes and do not survive a JavaScript number. */
  id: string;
  parent_id: string | null;
  position: number;
  title: string;
}

export interface Move {
  id: string;
  parent_id: string | null;
  position: number;
}

export type Placement = "before" | "after" | "inside";

export const MAX_DEPTH = 8;

/** Where on a row a drop lands, from how far down the row the pointer is (0 to 1). */
export function placementAt(ratio: number): Placement {
  if (ratio < 0.25) return "before";
  if (ratio > 0.75) return "after";
  return "inside";
}
const STEP = 10;

function siblings(nodes: TreeNode[], parent: string | null): TreeNode[] {
  return nodes
    .filter((n) => n.parent_id === parent)
    .sort((a, b) => a.position - b.position || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

/** Reading order with depth (a top-level entry is depth 1). Orphans show as top-level. */
export function rows<T extends TreeNode>(nodes: T[]): { node: T; depth: number }[] {
  const ids = new Set(nodes.map((n) => n.id));
  const out: { node: T; depth: number }[] = [];
  const walk = (parent: string | null, depth: number) => {
    const list = parent === null ? nodes.filter((n) => n.parent_id === null || !ids.has(n.parent_id)) : nodes.filter((n) => n.parent_id === parent);
    list
      .sort((a, b) => a.position - b.position || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
      .forEach((n) => {
        out.push({ node: n, depth });
        walk(n.id, depth + 1);
      });
  };
  walk(null, 1);
  return out;
}

/** Whether `id` is `ancestor` or sits below it. */
export function isWithin(nodes: TreeNode[], ancestor: string, id: string): boolean {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  let cur: string | null = id;
  for (let i = 0; cur !== null && i <= nodes.length; i++) {
    if (cur === ancestor) return true;
    cur = byId.get(cur)?.parent_id ?? null;
  }
  return false;
}

function depthOf(nodes: TreeNode[], id: string): number {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  let depth = 0;
  let cur: string | null = id;
  while (cur !== null && depth <= nodes.length) {
    depth++;
    cur = byId.get(cur)?.parent_id ?? null;
  }
  return depth;
}

function height(nodes: TreeNode[], id: string): number {
  const kids = nodes.filter((n) => n.parent_id === id);
  return 1 + kids.reduce((m, k) => Math.max(m, height(nodes, k.id)), 0);
}

/** Puts `dragId` into `parent` at index `at` of its (renumbered) children. */
function place(nodes: TreeNode[], dragId: string, parent: string | null, at: number): Move[] {
  const list = siblings(nodes, parent).filter((n) => n.id !== dragId);
  const index = Math.max(0, Math.min(at, list.length));
  const ordered = [...list.slice(0, index), { id: dragId } as TreeNode, ...list.slice(index)];
  const moves: Move[] = [];
  ordered.forEach((n, i) => {
    const position = (i + 1) * STEP;
    const current = nodes.find((x) => x.id === n.id);
    if (!current || current.parent_id !== parent || current.position !== position) {
      moves.push({ id: n.id, parent_id: parent, position });
    }
  });
  return moves;
}

/** The moves for dropping `dragId` before, after or inside `targetId`; empty when illegal or a no-op. */
export function movesForDrop(nodes: TreeNode[], dragId: string, targetId: string, placement: Placement): Move[] {
  if (dragId === targetId || isWithin(nodes, dragId, targetId)) return [];
  const target = nodes.find((n) => n.id === targetId);
  if (!target) return [];
  const parent = placement === "inside" ? target.id : target.parent_id;
  // How deep the new parent sits; the dragged subtree hangs below it.
  const parentDepth = placement === "inside" ? depthOf(nodes, target.id) : depthOf(nodes, target.id) - 1;
  if (parentDepth + height(nodes, dragId) > MAX_DEPTH) return [];
  const list = siblings(nodes, parent).filter((n) => n.id !== dragId);
  const at = placement === "inside" ? list.length : list.findIndex((n) => n.id === target.id) + (placement === "after" ? 1 : 0);
  return place(nodes, dragId, parent, at);
}

/** Moves an entry one place earlier among its siblings. */
export function moveUp(nodes: TreeNode[], id: string): Move[] {
  const node = nodes.find((n) => n.id === id);
  if (!node) return [];
  const list = siblings(nodes, node.parent_id);
  const i = list.findIndex((n) => n.id === id);
  return i <= 0 ? [] : place(nodes, id, node.parent_id, i - 1);
}

/** Moves an entry one place later among its siblings. */
export function moveDown(nodes: TreeNode[], id: string): Move[] {
  const node = nodes.find((n) => n.id === id);
  if (!node) return [];
  const list = siblings(nodes, node.parent_id);
  const i = list.findIndex((n) => n.id === id);
  return i < 0 || i >= list.length - 1 ? [] : place(nodes, id, node.parent_id, i + 1);
}

/** Makes an entry the last child of the sibling before it. */
export function indent(nodes: TreeNode[], id: string): Move[] {
  const node = nodes.find((n) => n.id === id);
  if (!node) return [];
  const list = siblings(nodes, node.parent_id);
  const i = list.findIndex((n) => n.id === id);
  const prev = list[i - 1];
  if (i <= 0 || !prev) return [];
  return movesForDrop(nodes, id, prev.id, "inside");
}

/** Makes an entry the sibling right after its parent. */
export function outdent(nodes: TreeNode[], id: string): Move[] {
  const node = nodes.find((n) => n.id === id);
  if (!node || node.parent_id === null) return [];
  return movesForDrop(nodes, id, node.parent_id, "after");
}
