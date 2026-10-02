import { describe, expect, it } from "vitest";

import {
  createBlock,
  defaultAttrs,
  isPluginKind,
  type Block,
  type BlockKind,
} from "@/components/editor/blocks";
import {
  blocksToDoc,
  docToBlocks,
  isNative,
} from "@/components/editor/canvas/serialize";

describe("plugin blocks in the editor", () => {
  it("tells a plugin kind from a core kind by its namespace", () => {
    expect(isPluginKind("bookshelf/rating")).toBe(true);
    expect(isPluginKind("a/b")).toBe(true);
    // Core kinds never contain a slash — that is the whole point of
    // requiring one, so a plugin can never shadow `paragraph`.
    expect(isPluginKind("paragraph")).toBe(false);
    expect(isPluginKind("media_text")).toBe(false);
    expect(isPluginKind("")).toBe(false);
  });

  it("gives an unknown plugin kind empty attrs rather than throwing", () => {
    const kind = "bookshelf/rating" as BlockKind;
    expect(defaultAttrs(kind)).toEqual({});
    const block = createBlock(kind);
    expect(block).toEqual({ kind, attrs: {}, children: [] });
  });

  it("round-trips a plugin block through the canvas without losing it", () => {
    // The editor keeps kinds it cannot edit natively as opaque atoms. If
    // that dropped a plugin block, opening a post in the editor and saving
    // it would silently delete the block.
    const original: Block = {
      kind: "bookshelf/rating" as BlockKind,
      attrs: { stars: 4, title: "Dune" },
      children: [],
    };
    // A plugin kind is never native, so it takes the opaque path.
    expect(isNative(original.kind)).toBe(false);
    const doc = blocksToDoc([original]);
    expect(docToBlocks(doc)).toEqual([original]);
  });
});
