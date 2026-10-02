import { describe, expect, it } from "vitest";
import {
  blocksToDoc,
  docToBlocks,
  isEmptyDoc,
} from "@/components/editor/canvas/serialize";
import {
  htmlToInline,
  inlineToHtml,
  inlineToText,
} from "@/components/editor/canvas/inline";
import { normalizeBlocks } from "@/components/editor/blocks";
import type { Block } from "@/components/editor/blocks";

const b = (
  kind: string,
  attrs: Record<string, unknown>,
  children: Block[] = [],
): Block => ({ kind: kind as Block["kind"], attrs, children });

describe("inline html", () => {
  it("round-trips every mark the server keeps", () => {
    // Mirrors vyasa_core::block::INLINE_TAGS.
    const html =
      'plain <strong>bold</strong> <em>italic</em> <s>struck</s> <u>under</u> <mark>lit</mark> H<sub>2</sub>O x<sup>2</sup> <code>mono</code> <a href="/x">link</a>';
    expect(inlineToHtml(htmlToInline(html))).toBe(html);
  });

  it("normalises the tag aliases the allowlist also accepts", () => {
    expect(inlineToHtml(htmlToInline("<b>b</b> <i>i</i> <del>d</del> <ins>i</ins>"))).toBe(
      "<strong>b</strong> <em>i</em> <s>d</s> <u>i</u>",
    );
  });

  it("escapes text that would otherwise be markup", () => {
    const nodes = htmlToInline("a &lt; b &amp; c");
    expect(inlineToText(nodes)).toBe("a < b & c");
    expect(inlineToHtml(nodes)).toBe("a &lt; b &amp; c");
  });

  it("keeps a link's title and merges adjacent identical marks", () => {
    const html = '<a href="/a" title="T">one</a>';
    expect(inlineToHtml(htmlToInline(html))).toBe(html);

    const merged = inlineToHtml([
      { type: "text", text: "bo", marks: [{ type: "bold" }] },
      { type: "text", text: "ld", marks: [{ type: "bold" }] },
    ]);
    expect(merged).toBe("<strong>bold</strong>");
  });

  it("drops tags outside the allowlist but keeps their text", () => {
    // <span> and <font> are not in the allowlist; their text must survive.
    const nodes = htmlToInline('keep <span class="x">this</span> <font>text</font>');
    expect(inlineToText(nodes)).toBe("keep this text");
    expect(inlineToHtml(nodes)).toBe("keep this text");
  });

  it("handles nested marks without losing either", () => {
    const html = '<a href="/x"><strong>bold link</strong></a>';
    const back = inlineToHtml(htmlToInline(html));
    expect(back).toContain("<a href=\"/x\">");
    expect(back).toContain("<strong>");
    expect(inlineToText(htmlToInline(back))).toBe("bold link");
  });

  it("preserves hard breaks", () => {
    expect(inlineToHtml(htmlToInline("one<br>two"))).toBe("one<br>two");
  });
});

describe("document round-trip", () => {
  it("preserves the text-shaped kinds in the server's format", () => {
    const blocks: Block[] = [
      b("heading", { level: 3, text: "Title <em>x</em>" }),
      b("paragraph", { text: "Hello <strong>world</strong>" }),
      b("list", { ordered: true }, [
        b("paragraph", { text: "one" }),
        b("paragraph", { text: "two" }),
      ]),
      b("quote", { text: "Quoted", citation: "Someone" }),
      b("code", { language: "rust", code: "fn main() {}" }),
      b("separator", {}),
    ];
    expect(docToBlocks(blocksToDoc(blocks))).toEqual(blocks);
  });

  it("round-trips a nested list as a list child of its parent", () => {
    // The renderer nests a `list` child inside the item before it.
    const blocks: Block[] = [
      b("list", { ordered: false }, [
        b("paragraph", { text: "outer" }),
        b("list", { ordered: true }, [b("paragraph", { text: "inner" })]),
        b("paragraph", { text: "after" }),
      ]),
    ];
    expect(docToBlocks(blocksToDoc(blocks))).toEqual(blocks);
  });

  it("edits images natively and keeps attributes it does not model", () => {
    const blocks: Block[] = [
      b("image", { url: "/api/v1/media/3/raw", alt: "A", caption: "Cap <em>x</em>", width: 400 }),
    ];
    const doc = blocksToDoc(blocks);
    expect(doc.content?.[0]?.type).toBe("vyImage");
    expect(docToBlocks(doc)).toEqual(blocks);
  });

  it("leaves out an image that has no file yet", () => {
    // The validator rejects a media block with neither url nor mediaId, so
    // an empty placeholder must not reach the server.
    const doc = {
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "a" }] },
        { type: "vyImage", attrs: { url: "", alt: "", pending: "blob:x" } },
      ],
    };
    expect(docToBlocks(doc).map((x) => x.kind)).toEqual(["paragraph"]);
  });

  it("edits tables natively: header row, cells, caption and extras", () => {
    const blocks: Block[] = [
      b("table", {
        header: ["Name", "Note"],
        rows: [["a", "<em>rich</em>"], ["b", ""]],
        caption: "Two rows",
        align: "center",
      }),
    ];
    const doc = blocksToDoc(blocks);
    expect(doc.content?.[0]?.type).toBe("table");
    expect(docToBlocks(doc)).toEqual(blocks);

    const headless: Block[] = [b("table", { rows: [["x", "y"]] })];
    expect(docToBlocks(blocksToDoc(headless))).toEqual(headless);
  });

  it("pads ragged table rows to the widest row", () => {
    const back = docToBlocks(blocksToDoc([b("table", { rows: [["a"], ["b", "c"]] })]));
    expect(back[0]?.attrs["rows"]).toEqual([["a", ""], ["b", "c"]]);
  });

  it("never loses a block it cannot edit natively", () => {
    // A gallery, an embed and a plugin-supplied kind all survive verbatim.
    const blocks: Block[] = [
      b("paragraph", { text: "before" }),
      b("gallery", {}, [b("image", { url: "/a.png", alt: "A" })]),
      b("embed", { url: "https://example.com/v" }),
      b("some_plugin_kind", { anything: { nested: true }, n: 42 }),
      b("paragraph", { text: "after" }),
    ];
    expect(docToBlocks(blocksToDoc(blocks))).toEqual(blocks);
  });

  it("keeps children of opaque container blocks", () => {
    const blocks: Block[] = [
      b("columns", { count: 2 }, [
        b("paragraph", { text: "left" }),
        b("paragraph", { text: "right" }),
      ]),
    ];
    const back = docToBlocks(blocksToDoc(blocks));
    expect(back).toEqual(blocks);
    expect(back[0]?.children).toHaveLength(2);
  });

  it("clamps heading levels into the range the server accepts", () => {
    // The API rejects level 1 and level 7; the canvas must not emit them.
    for (const [given, expected] of [
      [1, 2],
      [7, 6],
      [4, 4],
    ] as const) {
      const back = docToBlocks(blocksToDoc([b("heading", { level: given, text: "H" })]));
      expect(back[0]?.attrs["level"]).toBe(expected);
    }
  });

  it("writes a null language for plain code, never the string 'plain'", () => {
    const back = docToBlocks(blocksToDoc([b("code", { language: "plain", code: "x" })]));
    expect(back[0]?.attrs["language"]).toBeNull();
  });

  it("treats a lone empty paragraph as an empty document", () => {
    expect(isEmptyDoc(blocksToDoc([]))).toBe(true);
    expect(isEmptyDoc(blocksToDoc([b("paragraph", { text: "x" })]))).toBe(false);
  });

  it("survives a second round trip unchanged", () => {
    const blocks: Block[] = [
      b("paragraph", { text: 'a <a href="/l">link</a> here' }),
      b("image", { url: "/i.png", alt: "", caption: "cap" }),
      b("list", { ordered: false }, [b("paragraph", { text: "i" })]),
    ];
    const once = docToBlocks(blocksToDoc(blocks));
    const twice = docToBlocks(blocksToDoc(once));
    expect(twice).toEqual(once);
  });
});

describe("wire-format tolerance", () => {
  // The API omits `children` when a block has none. Code that walked the tree
  // assuming it was present crashed the editor with "o is not iterable" the
  // moment a real post was opened.
  it("fills in children the server left out", () => {
    const fromServer = [
      { kind: "paragraph", attrs: { text: "Secret draft" } },
      { kind: "group", attrs: {}, children: [{ kind: "paragraph", attrs: {} }] },
    ];
    const blocks = normalizeBlocks(fromServer);
    expect(blocks[0]?.children).toEqual([]);
    expect(blocks[1]?.children[0]?.children).toEqual([]);
  });

  it("survives junk instead of an array", () => {
    expect(normalizeBlocks(undefined)).toEqual([]);
    expect(normalizeBlocks(null)).toEqual([]);
    expect(normalizeBlocks({})).toEqual([]);
  });

  it("round-trips a block that arrived without children", () => {
    const blocks = normalizeBlocks([
      { kind: "paragraph", attrs: { text: "Secret draft" } },
    ]);
    expect(docToBlocks(blocksToDoc(blocks))).toEqual(blocks);
  });
});
