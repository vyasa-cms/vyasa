import { describe, expect, it } from "vitest";
import { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { buildExtensions } from "@/components/editor/canvas/extensions";
import { blocksToDoc, docToBlocks, type PMNode } from "@/components/editor/canvas/serialize";
import {
  currentBlock,
  duplicateBlock,
  locateBlock,
  moveBlock,
  removeBlock,
} from "@/components/editor/canvas/blockOps";
import type { Block } from "@/components/editor/blocks";

const b = (
  kind: string,
  attrs: Record<string, unknown>,
  children: Block[] = [],
): Block => ({ kind: kind as Block["kind"], attrs, children });

function open(blocks: Block[]): Editor {
  return new Editor({
    extensions: buildExtensions(),
    content: blocksToDoc(blocks) as unknown as Record<string, unknown>,
  });
}

function texts(editor: Editor): string[] {
  return docToBlocks(editor.getJSON() as unknown as PMNode).flatMap((x) =>
    x.kind === "columns"
      ? x.children.map((c) => `col:${String(c.attrs["text"])}`)
      : [String(x.attrs["text"])],
  );
}

describe("block operations", () => {
  it("moves a top-level block among its siblings and keeps the rest", () => {
    const editor = open([
      b("paragraph", { text: "one" }),
      b("paragraph", { text: "two" }),
      b("paragraph", { text: "three" }),
    ]);
    try {
      // Caret inside "two".
      editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 7)));
      const block = currentBlock(editor);
      expect(block?.index).toBe(1);
      expect(block?.count).toBe(3);
      expect(moveBlock(editor, block!, -1)).toBe(true);
      expect(texts(editor)).toEqual(["two", "one", "three"]);
      expect(moveBlock(editor, currentBlock(editor)!, -1)).toBe(false); // already first
    } finally {
      editor.destroy();
    }
  });

  it("treats a block inside a container as a sibling of its column mates", () => {
    const editor = open([
      b("paragraph", { text: "before" }),
      b("columns", {}, [b("paragraph", { text: "left" }), b("paragraph", { text: "right" })]),
    ]);
    try {
      // Find the position of "right" and place the caret in it.
      let pos = -1;
      editor.state.doc.descendants((node, p) => {
        if (node.isText && node.text === "right") pos = p;
      });
      expect(pos).toBeGreaterThan(0);
      editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos + 1)));
      const block = locateBlock(editor, pos + 1);
      expect(block?.node.textContent).toBe("right");
      expect(block?.index).toBe(1);
      expect(block?.count).toBe(2);
      moveBlock(editor, block!, -1);
      expect(texts(editor)).toEqual(["before", "col:right", "col:left"]);
    } finally {
      editor.destroy();
    }
  });

  it("duplicates and removes the current block", () => {
    const editor = open([b("paragraph", { text: "solo" }), b("paragraph", { text: "tail" })]);
    try {
      editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
      duplicateBlock(editor, currentBlock(editor)!);
      expect(texts(editor)).toEqual(["solo", "solo", "tail"]);
      removeBlock(editor, locateBlock(editor, 1)!);
      expect(texts(editor)).toEqual(["solo", "tail"]);
    } finally {
      editor.destroy();
    }
  });
});
