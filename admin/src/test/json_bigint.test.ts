import { describe, expect, it } from "vitest";
import {
  parseJsonPreservingIds,
  quoteUnsafeIntegers,
} from "@/api/json-bigint";

describe("snowflake-safe JSON", () => {
  it("keeps a 64-bit id exact instead of rounding it", () => {
    // The real failure: this id round-tripped to 350710414700445700 and the
    // server then reported "post not found".
    const parsed = parseJsonPreservingIds<{ id: string }>(
      '{"id":350710414700445696}',
    );
    expect(parsed.id).toBe("350710414700445696");
    expect(String(parsed.id)).toBe("350710414700445696");

    // Demonstrates why: the plain parse cannot express it.
    expect(String(JSON.parse('{"id":350710414700445696}').id)).toBe(
      "350710414700445700",
    );
  });

  it("leaves ordinary numbers as numbers", () => {
    const parsed = parseJsonPreservingIds<{
      total: number;
      page: number;
      ratio: number;
      neg: number;
    }>('{"total":21,"page":1,"ratio":1.5,"neg":-3}');
    expect(parsed.total).toBe(21);
    expect(parsed.page).toBe(1);
    expect(parsed.ratio).toBe(1.5);
    expect(parsed.neg).toBe(-3);
  });

  it("does not rewrite long digits inside string values", () => {
    const json = '{"title":"call 350710414700445696 now","id":350710414700445696}';
    const parsed = parseJsonPreservingIds<{ title: string; id: string }>(json);
    expect(parsed.title).toBe("call 350710414700445696 now");
    expect(parsed.id).toBe("350710414700445696");
  });

  it("handles escaped quotes inside strings", () => {
    const json = '{"t":"a \\" 350710414700445696","id":350710414700445696}';
    const parsed = parseJsonPreservingIds<{ t: string; id: string }>(json);
    expect(parsed.t).toBe('a " 350710414700445696');
    expect(parsed.id).toBe("350710414700445696");
  });

  it("handles arrays, nesting and negatives", () => {
    const json =
      '{"items":[{"id":350710414700445696,"author_id":-350710414700445697}],"n":[1,2,3]}';
    const parsed = parseJsonPreservingIds<{
      items: { id: string; author_id: string }[];
      n: number[];
    }>(json);
    expect(parsed.items[0]?.id).toBe("350710414700445696");
    expect(parsed.items[0]?.author_id).toBe("-350710414700445697");
    expect(parsed.n).toEqual([1, 2, 3]);
  });

  it("leaves exponents and floats untouched", () => {
    expect(quoteUnsafeIntegers('{"a":1e21,"b":1.25,"c":-0.5}')).toBe(
      '{"a":1e21,"b":1.25,"c":-0.5}',
    );
  });

  it("is a no-op for payloads with no oversized integers", () => {
    const json = '{"a":[1,2],"b":"x","c":null,"d":true}';
    expect(quoteUnsafeIntegers(json)).toBe(json);
    expect(parseJsonPreservingIds(json)).toEqual(JSON.parse(json));
  });
});
