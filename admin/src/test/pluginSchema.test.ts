import { describe, expect, it } from "vitest";
import { blankRow, containerOf, fieldsFromSchema, syncChildren, withAttr } from "@/lib/pluginSchema";

describe("fieldsFromSchema", () => {
  it("maps the supported subset and labels from keys", () => {
    const fields = fieldsFromSchema({
      type: "object",
      properties: {
        title: { type: "string" },
        code: { type: "string", format: "code", title: "Source" },
        note: { type: "string", format: "inline-html" },
        count: { type: "integer" },
        on: { type: "boolean" },
        tone: { type: "string", enum: ["a", "b"] },
        labels: { type: "array", items: { type: "string" } },
        rows: { type: "array", items: { type: "object", properties: { name: { type: "string" }, required: { type: "boolean" } } } },
        odd: { type: "object" },
      },
    });
    expect(fields.map((f) => [f.key, f.kind, f.label])).toEqual([
      ["title", "text", "Title"],
      ["code", "code", "Source"],
      ["note", "multiline", "Note"],
      ["count", "number", "Count"],
      ["on", "boolean", "On"],
      ["tone", "choice", "Tone"],
      ["labels", "list", "Labels"],
      ["rows", "rows", "Rows"],
      ["odd", "json", "Odd"],
    ]);
    expect(fields[5]?.options).toEqual(["a", "b"]);
    expect(fields[7]?.columns?.map((c) => c.kind)).toEqual(["text", "boolean"]);
  });

  it("is empty without a usable schema", () => {
    expect(fieldsFromSchema(undefined)).toEqual([]);
    expect(fieldsFromSchema({ type: "string" })).toEqual([]);
    expect(fieldsFromSchema({ type: "object" })).toEqual([]);
  });
});

describe("withAttr", () => {
  it("sets without mutating and removes empty values", () => {
    const a = { x: "1" };
    expect(withAttr(a, "y", "2")).toEqual({ x: "1", y: "2" });
    expect(a).toEqual({ x: "1" });
    expect(withAttr({ x: "1" }, "x", "")).toEqual({});
    expect(withAttr({ x: ["a"] }, "x", [])).toEqual({});
    expect(withAttr({ x: 1 }, "x", false)).toEqual({ x: false });
  });
});

describe("blankRow", () => {
  it("fills each column by kind", () => {
    expect(blankRow([{ key: "n", label: "N", kind: "text" }, { key: "r", label: "R", kind: "boolean" }, { key: "c", label: "C", kind: "number" }])).toEqual({ n: "", r: false, c: 0 });
  });
});

describe("syncChildren", () => {
  const make = () => ({ children: [] as unknown[] });
  it("grows with new empty children and keeps the old ones", () => {
    const old = [{ children: [1] }];
    const out = syncChildren(old, 3, make);
    expect(out).toHaveLength(3);
    expect(out[0]).toBe(old[0]);
  });
  it("shrinks only empty trailing children unless forced", () => {
    const kids = [{ children: [1] }, { children: [] }, { children: [2] }];
    expect(syncChildren(kids, 2, make)).toBe(kids);
    expect(syncChildren(kids, 2, make, true)).toHaveLength(2);
    expect(syncChildren([{ children: [1] }, { children: [] }], 1, make)).toHaveLength(1);
  });
  it("returns the same array when the count already matches", () => {
    const kids = [{ children: [] }];
    expect(syncChildren(kids, 1, make)).toBe(kids);
  });
});

describe("containerOf", () => {
  it("reads child and labels", () => {
    expect(containerOf({ container: { child: "group", labels: "labels" } })).toEqual({ child: "group", labels: "labels" });
    expect(containerOf({ container: { child: "group" } })).toEqual({ child: "group" });
    expect(containerOf({})).toBeUndefined();
    expect(containerOf({ container: { labels: "x" } })).toBeUndefined();
    expect(containerOf(undefined)).toBeUndefined();
  });
});
