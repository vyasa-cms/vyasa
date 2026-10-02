import type { Editor } from "@tiptap/core";
import type { Node as PMNode } from "@tiptap/pm/model";
import { NodeSelection } from "@tiptap/pm/state";

/**
 * Whole-block operations shared by the drag handle, the mobile toolbar and
 * keyboard users. A "block" here is a direct child of the document or of an
 * editable container, so a paragraph inside a two-column layout moves among
 * its column siblings rather than being invisible to these controls.
 */

export interface BlockTarget {
  /** Start position of the block. */
  pos: number;
  node: PMNode;
  /** Index among its parent's children. */
  index: number;
  /** How many siblings the parent has. */
  count: number;
}

const HOSTS = new Set(["doc", "rpContainer"]);

/** The block that contains `pos`, or that starts at it. */
export function locateBlock(editor: Editor, pos: number): BlockTarget | null {
  const { doc } = editor.state;
  if (pos < 0 || pos > doc.content.size) return null;
  const $pos = doc.resolve(pos);
  for (let depth = $pos.depth; depth >= 0; depth -= 1) {
    const parent = $pos.node(depth);
    if (!HOSTS.has(parent.type.name)) continue;
    if (depth < $pos.depth) {
      return {
        pos: $pos.before(depth + 1),
        node: $pos.node(depth + 1),
        index: $pos.index(depth),
        count: parent.childCount,
      };
    }
    const after = $pos.nodeAfter;
    if (after !== null) {
      return { pos, node: after, index: $pos.index(depth), count: parent.childCount };
    }
    const before = $pos.nodeBefore;
    if (before !== null) {
      return {
        pos: pos - before.nodeSize,
        node: before,
        index: $pos.index(depth) - 1,
        count: parent.childCount,
      };
    }
    return null;
  }
  return null;
}

/** The block the caret (or node selection) is in. */
export function currentBlock(editor: Editor): BlockTarget | null {
  const { selection } = editor.state;
  if (selection instanceof NodeSelection) return locateBlock(editor, selection.from);
  return locateBlock(editor, selection.$head.pos);
}

export function selectBlock(editor: Editor, target: BlockTarget): void {
  const tr = editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, target.pos));
  editor.view.dispatch(tr);
}

/** Swaps the block with its previous (-1) or next (+1) sibling. */
export function moveBlock(editor: Editor, target: BlockTarget, direction: -1 | 1): boolean {
  const { doc } = editor.state;
  const to = target.index + direction;
  if (to < 0 || to >= target.count) return false;
  const $start = doc.resolve(target.pos);
  const parent = $start.parent;
  const sibling = parent.child(to);
  const start = target.pos;
  const end = start + target.node.nodeSize;
  let tr = editor.state.tr.delete(start, end);
  // After the deletion the next sibling begins where this block began, and
  // the previous one is unaffected.
  const insertAt = direction === -1 ? start - sibling.nodeSize : start + sibling.nodeSize;
  tr = tr.insert(insertAt, target.node);
  tr = tr.setSelection(NodeSelection.create(tr.doc, insertAt));
  editor.view.dispatch(tr.scrollIntoView());
  return true;
}

export function duplicateBlock(editor: Editor, target: BlockTarget): void {
  const copy = target.node.copy(target.node.content);
  editor.view.dispatch(
    editor.state.tr.insert(target.pos + target.node.nodeSize, copy).scrollIntoView(),
  );
}

export function removeBlock(editor: Editor, target: BlockTarget): void {
  editor.view.dispatch(
    editor.state.tr.delete(target.pos, target.pos + target.node.nodeSize).scrollIntoView(),
  );
}
