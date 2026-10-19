import { describe, expect, it } from "vitest";
import { indent, isWithin, movesForDrop, moveDown, moveUp, outdent, placementAt, rows, type TreeNode } from "@/lib/tree";

const n = (id: string, parent: string | null, position: number, title = id): TreeNode => ({ id, parent_id: parent, position, title });

// a (10)
//   a1 (10)
//   a2 (20)
// b (20)
// c (30)
const sample = [n("a", null, 10), n("a1", "a", 10), n("a2", "a", 20), n("b", null, 20), n("c", null, 30)];

describe("rows", () => {
  it("is reading order with depth", () => {
    expect(rows(sample).map((r) => [r.node.id, r.depth])).toEqual([["a", 1], ["a1", 2], ["a2", 2], ["b", 1], ["c", 1]]);
  });
  it("shows an orphan at the top instead of losing it", () => {
    expect(rows([n("x", "gone", 1)]).map((r) => [r.node.id, r.depth])).toEqual([["x", 1]]);
  });
});

describe("isWithin", () => {
  it("includes itself and descendants only", () => {
    expect(isWithin(sample, "a", "a")).toBe(true);
    expect(isWithin(sample, "a", "a2")).toBe(true);
    expect(isWithin(sample, "a", "b")).toBe(false);
  });
});

describe("movesForDrop", () => {
  it("drops after a sibling and renumbers in tens", () => {
    expect(movesForDrop(sample, "c", "a", "after")).toEqual([
      { id: "c", parent_id: null, position: 20 },
      { id: "b", parent_id: null, position: 30 },
    ]);
  });
  it("drops before the first sibling", () => {
    expect(movesForDrop(sample, "c", "a", "before")).toEqual([
      { id: "c", parent_id: null, position: 10 },
      { id: "a", parent_id: null, position: 20 },
      { id: "b", parent_id: null, position: 30 },
    ]);
  });
  it("drops inside as the last child", () => {
    expect(movesForDrop(sample, "b", "a", "inside")).toEqual([{ id: "b", parent_id: "a", position: 30 }]);
  });
  it("refuses a drop onto itself or its own descendant", () => {
    expect(movesForDrop(sample, "a", "a", "inside")).toEqual([]);
    expect(movesForDrop(sample, "a", "a1", "after")).toEqual([]);
  });
  it("is empty when nothing would change", () => {
    expect(movesForDrop(sample, "b", "a", "after")).toEqual([]);
  });
  it("refuses to nest past the depth limit", () => {
    let deep: TreeNode[] = [n("d1", null, 10)];
    for (let i = 2; i <= 8; i++) deep = [...deep, n(`d${i}`, `d${i - 1}`, 10)];
    deep = [...deep, n("x", null, 20)];
    expect(movesForDrop(deep, "x", "d8", "inside")).toEqual([]);
    expect(movesForDrop(deep, "x", "d7", "inside")).not.toEqual([]);
  });
  it("counts the dragged subtree's height", () => {
    let deep: TreeNode[] = [n("d1", null, 10), n("d2", "d1", 10), n("d3", "d2", 10), n("d4", "d3", 10), n("d5", "d4", 10), n("d6", "d5", 10), n("d7", "d6", 10)];
    deep = [...deep, n("t", null, 20), n("t1", "t", 10), n("t2", "t1", 10)];
    // t has height 3; under d7 (depth 7) it would reach depth 10.
    expect(movesForDrop(deep, "t", "d7", "inside")).toEqual([]);
    expect(movesForDrop(deep, "t", "d5", "inside")).not.toEqual([]);
  });
});

describe("keyboard moves", () => {
  it("moves up and down among siblings", () => {
    expect(moveUp(sample, "b")).toEqual([
      { id: "b", parent_id: null, position: 10 },
      { id: "a", parent_id: null, position: 20 },
    ]);
    expect(moveUp(sample, "a")).toEqual([]);
    expect(moveDown(sample, "c")).toEqual([]);
    expect(moveDown(sample, "a1")).toEqual([
      { id: "a2", parent_id: "a", position: 10 },
      { id: "a1", parent_id: "a", position: 20 },
    ]);
  });
  it("indents under the previous sibling and outdents after the parent", () => {
    expect(indent(sample, "b")).toEqual([{ id: "b", parent_id: "a", position: 30 }]);
    expect(indent(sample, "a")).toEqual([]);
    expect(outdent(sample, "a2")).toEqual([
      { id: "a2", parent_id: null, position: 20 },
      { id: "b", parent_id: null, position: 30 },
      { id: "c", parent_id: null, position: 40 },
    ]);
    expect(outdent(sample, "a")).toEqual([]);
  });
});

describe("placementAt", () => {
  it("is before in the top quarter, after in the bottom quarter, inside between", () => {
    expect([0, 0.24, 0.25, 0.5, 0.75, 0.76, 1].map(placementAt)).toEqual(["before", "before", "inside", "inside", "inside", "after", "after"]);
  });
});
