import { describe, expect, it } from "vitest";
import { Editor } from "@tiptap/core";
import { buildExtensions } from "@/components/editor/canvas/extensions";
import {
  blocksToDoc,
  docToBlocks,
  type PMNode,
} from "@/components/editor/canvas/serialize";
import type { Block } from "@/components/editor/blocks";

const b = (
  kind: string,
  attrs: Record<string, unknown>,
  children: Block[] = [],
): Block => ({ kind: kind as Block["kind"], attrs, children });

/**
 * Push blocks through a real TipTap editor and read them back. This is the
 * test that matters: ProseMirror silently discards attributes a node does not
 * declare, so a serializer that round-trips as plain JSON can still lose data
 * once the schema is involved.
 */
function throughEditor(blocks: Block[]): Block[] {
  const editor = new Editor({
    extensions: buildExtensions(),
    content: blocksToDoc(blocks) as unknown as Record<string, unknown>,
  });
  try {
    return docToBlocks(editor.getJSON() as unknown as PMNode);
  } finally {
    editor.destroy();
  }
}

describe("canvas schema", () => {
  it("keeps text blocks intact through the real schema", () => {
    const blocks: Block[] = [
      b("heading", { level: 2, text: "A heading" }),
      b("paragraph", { text: "Some <strong>bold</strong> text" }),
      b("list", { ordered: false }, [
        b("paragraph", { text: "one" }),
        b("paragraph", { text: "two" }),
      ]),
      b("code", { language: "rust", code: "fn main() {}" }),
      b("separator", {}),
      b("image", { url: "/api/v1/media/1/raw", alt: "One", caption: "" }),
      b("table", { header: ["h"], rows: [["c"]] }),
    ];
    expect(throughEditor(blocks)).toEqual(blocks);
  });

  it("keeps a nested list nested through the real schema", () => {
    const blocks: Block[] = [
      b("list", { ordered: false }, [
        b("paragraph", { text: "outer" }),
        b("list", { ordered: false }, [b("paragraph", { text: "inner" })]),
      ]),
    ];
    expect(throughEditor(blocks)).toEqual(blocks);
  });

  it("keeps a quote's citation", () => {
    const blocks: Block[] = [b("quote", { text: "Quoted", citation: "Ada" })];
    const back = throughEditor(blocks);
    expect(back[0]?.attrs["citation"]).toBe("Ada");
    expect(back[0]?.attrs["text"]).toBe("Quoted");
  });

  it("makes layout containers editable while preserving their children", () => {
    const blocks: Block[] = [
      b("columns", { count: 2 }, [
        b("paragraph", { text: "left" }),
        b("paragraph", { text: "right" }),
      ]),
      b("group", {}, [b("heading", { level: 3, text: "Inside" })]),
    ];
    expect(throughEditor(blocks)).toEqual(blocks);
  });

  it("round-trips a container nested inside a container", () => {
    const blocks: Block[] = [
      b("group", {}, [
        b("columns", { count: 2 }, [
          b("paragraph", { text: "a" }),
          b("paragraph", { text: "b" }),
        ]),
      ]),
    ];
    expect(throughEditor(blocks)).toEqual(blocks);
  });

  it("keeps an opaque block inside an editable container", () => {
    const blocks: Block[] = [
      b("group", {}, [
        b("paragraph", { text: "caption above" }),
        b("gallery", {}, [b("image", { url: "/g.png", alt: "" })]),
      ]),
    ];
    expect(throughEditor(blocks)).toEqual(blocks);
  });

  it("keeps blocks the canvas cannot edit, including their children", () => {
    const blocks: Block[] = [
      b("paragraph", { text: "before" }),
      b("gallery", {}, [b("image", { url: "/a.png", alt: "A" })]),
      b("paragraph", { text: "after" }),
    ];
    expect(throughEditor(blocks)).toEqual(blocks);
  });

  it("never emits a heading level the API rejects", () => {
    // validate.rs enforces 2..=6.
    for (const level of [1, 2, 6, 7]) {
      const back = throughEditor([b("heading", { level, text: "H" })]);
      const got = Number(back[0]?.attrs["level"]);
      expect(got).toBeGreaterThanOrEqual(2);
      expect(got).toBeLessThanOrEqual(6);
    }
  });

  it("keeps every mark the server's allowlist keeps", () => {
    const back = throughEditor([
      b("paragraph", {
        text: "<s>s</s> <u>u</u> <mark>m</mark> <sub>b</sub> <sup>p</sup> <strong>st</strong> <em>e</em> <code>c</code>",
      }),
    ]);
    expect(back[0]?.attrs["text"]).toBe(
      "<s>s</s> <u>u</u> <mark>m</mark> <sub>b</sub> <sup>p</sup> <strong>st</strong> <em>e</em> <code>c</code>",
    );
  });

  it("offers exactly the marks the server's allowlist keeps", () => {
    // vyasa_core::block::INLINE_TAGS is the contract: a, b, i, em, strong,
    // code, br, s, del, ins, u, mark, sub, sup. Every mark here maps onto
    // one of those tags; a mark beyond them would let an author apply
    // formatting that vanishes on publish.
    const editor = new Editor({ extensions: buildExtensions() });
    try {
      expect(Object.keys(editor.schema.marks).sort()).toEqual([
        "bold",
        "code",
        "highlight",
        "italic",
        "link",
        "strike",
        "subscript",
        "superscript",
        "underline",
      ]);
    } finally {
      editor.destroy();
    }
  });

  it("drops a span, which is outside the allowlist, but keeps its text", () => {
    const back = throughEditor([b("paragraph", { text: 'keep <span style="color:red">this</span>' })]);
    expect(back[0]?.attrs["text"]).toBe("keep this");
  });

  it("survives repeated open-and-save cycles unchanged", () => {
    const blocks: Block[] = [
      b("paragraph", { text: 'link to <a href="/x">somewhere</a>' }),
      b("image", { url: "/i.png", alt: "alt", caption: "cap" }),
      b("quote", { text: "Q", citation: "C" }),
    ];
    const once = throughEditor(blocks);
    const twice = throughEditor(once);
    const thrice = throughEditor(twice);
    expect(twice).toEqual(once);
    expect(thrice).toEqual(once);
  });
});
