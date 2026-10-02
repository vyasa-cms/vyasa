import { describe, expect, it } from "vitest";
import {
  countWords,
  createBlock,
  normalizeBlocks,
  outlineOf,
  plainTextOf,
  readingMinutes,
} from "@/components/editor/blocks";
import { targetOwnsUndo } from "@/components/editor/PostEditor";

describe("legacy attribute upgrade", () => {
  // An earlier admin build wrote shapes the renderer never read. They are
  // rewritten on load so nothing already saved is lost or rendered empty.
  it("turns a list's items into paragraph children with an ordered flag", () => {
    const [list] = normalizeBlocks([
      { kind: "list", attrs: { style: "ordered", items: ["a", "b"] } },
    ]);
    expect(list?.attrs).toEqual({ ordered: true });
    expect(list?.children.map((c) => c.attrs["text"])).toEqual(["a", "b"]);
  });

  it("turns a gallery's images into image children", () => {
    const [gallery] = normalizeBlocks([
      { kind: "gallery", attrs: { images: [{ url: "/a.png", alt: "A" }, { url: "/b.png" }] } },
    ]);
    expect(gallery?.attrs).toEqual({});
    expect(gallery?.children).toEqual([
      { kind: "image", attrs: { url: "/a.png", alt: "A" }, children: [] },
      { kind: "image", attrs: { url: "/b.png", alt: "" }, children: [] },
    ]);
  });

  it("renames button text/url to label/href, which the validator requires", () => {
    const [button] = normalizeBlocks([
      { kind: "button", attrs: { text: "Go", url: "/x", style: "primary" } },
    ]);
    expect(button?.attrs).toEqual({ label: "Go", href: "/x", style: "primary" });
  });

  it("moves a table's has_header flag into a real header row", () => {
    const [table] = normalizeBlocks([
      { kind: "table", attrs: { rows: [["h1", "h2"], ["a", "b"]], has_header: true } },
    ]);
    expect(table?.attrs).toEqual({ header: ["h1", "h2"], rows: [["a", "b"]] });
  });

  it("uses a file's fileName as its caption and clears a 'plain' code language", () => {
    const blocks = normalizeBlocks([
      { kind: "file", attrs: { url: "/f.pdf", fileName: "Spec" } },
      { kind: "code", attrs: { language: "plain", code: "x" } },
    ]);
    expect(blocks[0]?.attrs).toEqual({ url: "/f.pdf", caption: "Spec" });
    expect(blocks[1]?.attrs).toEqual({ language: null, code: "x" });
  });

  it("leaves canonical blocks alone", () => {
    const canonical = [
      { kind: "list", attrs: { ordered: false }, children: [{ kind: "paragraph", attrs: { text: "i" }, children: [] }] },
      { kind: "button", attrs: { label: "L", href: "/" }, children: [] },
    ];
    expect(normalizeBlocks(canonical)).toEqual(canonical);
  });

  it("upgrades inside containers too", () => {
    const [group] = normalizeBlocks([
      { kind: "group", attrs: {}, children: [{ kind: "list", attrs: { items: ["x"] } }] },
    ]);
    expect(group?.children[0]?.children[0]?.attrs["text"]).toBe("x");
  });
});

describe("new blocks", () => {
  it("start in the server's shape with something to type into", () => {
    expect(createBlock("list")).toEqual({
      kind: "list",
      attrs: { ordered: false },
      children: [{ kind: "paragraph", attrs: { text: "" }, children: [] }],
    });
    expect(createBlock("button").attrs).toEqual({ label: "Learn more", href: "/" });
    expect(createBlock("buttons").children[0]?.kind).toBe("button");
    expect(createBlock("code").attrs["language"]).toBeNull();
  });
});

describe("text helpers", () => {
  const doc = normalizeBlocks([
    { kind: "heading", attrs: { level: 2, text: "One <em>two</em>" } },
    { kind: "paragraph", attrs: { text: "three four&nbsp;five" } },
    { kind: "list", attrs: { ordered: false }, children: [{ kind: "paragraph", attrs: { text: "six" } }] },
    { kind: "table", attrs: { header: ["seven"], rows: [["eight", "nine"]] } },
    { kind: "group", attrs: {}, children: [{ kind: "heading", attrs: { level: 4, text: "ten" } }] },
    { kind: "image", attrs: { url: "/x.png", alt: "not counted", caption: "eleven" } },
  ]);

  it("counts the words an author wrote, tags and entities aside", () => {
    expect(countWords(doc)).toBe(11);
    expect(plainTextOf(doc)).toContain("three four five");
  });

  it("estimates reading time at 200 words a minute, never under a minute", () => {
    expect(readingMinutes(0)).toBe(1);
    expect(readingMinutes(199)).toBe(1);
    expect(readingMinutes(1000)).toBe(5);
  });

  it("builds an outline from headings, including nested ones", () => {
    expect(outlineOf(doc)).toEqual([
      { level: 2, text: "One two" },
      { level: 4, text: "ten" },
    ]);
  });
});

describe("undo ownership", () => {
  // Ctrl+Z in the canvas, the title or a textarea belongs to that field;
  // the block-level history only steps in elsewhere.
  it("leaves fields with their own undo alone", () => {
    const pm = document.createElement("div");
    pm.className = "ProseMirror";
    const inner = document.createElement("p");
    pm.append(inner);
    expect(targetOwnsUndo(inner)).toBe(true);
    expect(targetOwnsUndo(document.createElement("input"))).toBe(true);
    expect(targetOwnsUndo(document.createElement("textarea"))).toBe(true);
    expect(targetOwnsUndo(document.createElement("button"))).toBe(false);
    expect(targetOwnsUndo(null)).toBe(false);
  });
});
