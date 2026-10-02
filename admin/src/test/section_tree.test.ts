import { describe, expect, it } from "vitest";
import {
  allIds,
  appendChild,
  canIndent,
  canOutdent,
  findSection,
  flatten,
  indentSection,
  insertSection,
  moveSection,
  outdentSection,
  pathTo,
  removeSection,
  uniqueId,
  updateSection,
  insertRelativeTo,
} from "@/components/studio/sectionTree";
import type { Section, Vocabulary } from "@/api/themes";

const vocab: Vocabulary = {
  static_regions: [{ kind: "header", description: "" }],
  blocks: [
    { kind: "band", description: "", settings_schema: {}, container: true },
    { kind: "columns", description: "", settings_schema: {}, container: true },
    { kind: "hero", description: "", settings_schema: {}, container: false },
    { kind: "cta-band", description: "", settings_schema: {}, container: false },
  ],
  templates: ["index", "single", "archive", "page", "search", "not-found"],
} as unknown as Vocabulary;

const s = (id: string, kind: string, children?: Section[]): Section =>
  children === undefined ? { id, kind } : { id, kind, children };

/** hero, band[ cta ], footer */
function tree(): Section[] {
  return [s("hero", "hero"), s("band", "band", [s("cta", "cta-band")]), s("footer", "header")];
}

describe("reading the tree", () => {
  it("flattens depth-first with depths", () => {
    expect(flatten(tree()).map((n) => [n.section.id, n.depth])).toEqual([
      ["hero", 0],
      ["band", 0],
      ["cta", 1],
      ["footer", 0],
    ]);
  });

  it("collects ids across the whole tree, not just the top", () => {
    // Ids must be unique template-wide, so a nested id counts as taken.
    expect(allIds(tree())).toEqual(["hero", "band", "cta", "footer"]);
  });

  it("finds nested sections and their ancestry", () => {
    expect(findSection(tree(), "cta")?.kind).toBe("cta-band");
    expect(pathTo(tree(), "cta")).toEqual(["band"]);
    expect(pathTo(tree(), "hero")).toEqual([]);
    expect(pathTo(tree(), "nope")).toBeNull();
  });

  it("derives an id that is free anywhere in the tree", () => {
    expect(uniqueId("hero", allIds(tree()))).toBe("hero-2");
    expect(uniqueId("faq", allIds(tree()))).toBe("faq");
  });
});

describe("editing the tree", () => {
  it("updates a nested section without disturbing its siblings", () => {
    const next = updateSection(tree(), "cta", (x) => ({ ...x, settings: { a: 1 } }));
    expect(findSection(next, "cta")?.settings).toEqual({ a: 1 });
    expect(allIds(next)).toEqual(allIds(tree()));
  });

  it("removes a nested section and leaves its parent", () => {
    const next = removeSection(tree(), "cta");
    expect(allIds(next)).toEqual(["hero", "band", "footer"]);
    expect(findSection(next, "band")?.children).toEqual([]);
  });

  it("inserts after a section at any depth", () => {
    const next = insertSection(tree(), s("new", "hero"), "cta");
    expect(findSection(next, "band")?.children?.map((c) => c.id)).toEqual(["cta", "new"]);
  });

  it("appends into a container", () => {
    const next = appendChild(tree(), "band", s("second", "cta-band"));
    expect(findSection(next, "band")?.children?.map((c) => c.id)).toEqual(["cta", "second"]);
  });
});

describe("moving within siblings", () => {
  it("reorders top-level sections", () => {
    expect(allIds(moveSection(tree(), "band", -1))).toEqual([
      "band",
      "cta",
      "hero",
      "footer",
    ]);
  });

  it("does nothing at the ends", () => {
    expect(allIds(moveSection(tree(), "hero", -1))).toEqual(allIds(tree()));
    expect(allIds(moveSection(tree(), "footer", 1))).toEqual(allIds(tree()));
  });

  it("never moves a section out of its parent", () => {
    // `cta` is an only child, so up and down are both no-ops rather than
    // silently promoting it to the top level.
    const up = moveSection(tree(), "cta", -1);
    expect(pathTo(up, "cta")).toEqual(["band"]);
    const down = moveSection(tree(), "cta", 1);
    expect(pathTo(down, "cta")).toEqual(["band"]);
  });

  it("reorders siblings inside a container", () => {
    const start = [s("band", "band", [s("a", "hero"), s("b", "hero")])];
    const next = moveSection(start, "b", -1);
    expect(findSection(next, "band")?.children?.map((c) => c.id)).toEqual(["b", "a"]);
  });
});

describe("nesting", () => {
  it("nests a section into the container above it", () => {
    const start = [s("band", "band"), s("cta", "cta-band")];
    expect(canIndent(start, "cta", vocab)).toBe(true);
    const next = indentSection(start, "cta", vocab);
    expect(pathTo(next, "cta")).toEqual(["band"]);
    expect(next).toHaveLength(1);
  });

  it("refuses to nest into a leaf", () => {
    const start = [s("hero", "hero"), s("cta", "cta-band")];
    expect(canIndent(start, "cta", vocab)).toBe(false);
    expect(indentSection(start, "cta", vocab)).toEqual(start);
  });

  it("refuses to nest the first section", () => {
    expect(canIndent(tree(), "hero", vocab)).toBe(false);
  });

  it("lifts a section out, placing it after its old parent", () => {
    expect(canOutdent(tree(), "cta")).toBe(true);
    const next = outdentSection(tree(), "cta");
    expect(allIds(next)).toEqual(["hero", "band", "cta", "footer"]);
    expect(pathTo(next, "cta")).toEqual([]);
  });

  it("will not outdent something already at the top", () => {
    expect(canOutdent(tree(), "hero")).toBe(false);
    expect(outdentSection(tree(), "hero")).toEqual(tree());
  });

  it("survives a nest-then-unnest round trip", () => {
    const start = [s("band", "band"), s("cta", "cta-band")];
    const there = indentSection(start, "cta", vocab);
    const back = outdentSection(there, "cta");
    expect(allIds(back)).toEqual(["band", "cta"]);
  });

  it("keeps a subtree intact when its container moves", () => {
    const start = [
      s("first", "hero"),
      s("band", "band", [s("a", "cta-band"), s("b", "cta-band")]),
    ];
    const next = moveSection(start, "band", -1);
    expect(findSection(next, "band")?.children?.map((c) => c.id)).toEqual(["a", "b"]);
    expect(allIds(next)).toEqual(["band", "a", "b", "first"]);
  });
});

describe("insertRelativeTo", () => {
  const tree = [
    { id: "a", kind: "hero" },
    { id: "band", kind: "band", children: [{ id: "b", kind: "cta-band" }] },
  ];

  it("lands exactly where the insertion line pointed, either side", () => {
    const before = insertRelativeTo(tree, { id: "x", kind: "image" }, "band", "before");
    expect(before.map((s) => s.id)).toEqual(["a", "x", "band"]);
    const after = insertRelativeTo(tree, { id: "x", kind: "image" }, "a", "after");
    expect(after.map((s) => s.id)).toEqual(["a", "x", "band"]);
  });

  it("reaches nested targets inside containers", () => {
    const next = insertRelativeTo(tree, { id: "x", kind: "image" }, "b", "after");
    expect(next[1]?.children?.map((s) => s.id)).toEqual(["b", "x"]);
  });
});

