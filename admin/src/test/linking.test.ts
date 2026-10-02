import { describe, expect, it } from "vitest";
import { appendFurtherReading, linkPhrase } from "@/components/editor/linking";
import type { Block } from "@/components/editor/blocks";

const p = (text: string): Block => ({ kind: "paragraph", attrs: { text }, children: [] });

describe("linkPhrase", () => {
  it("wraps the first occurrence, keeping the draft's own casing", () => {
    const next = linkPhrase(
      [p("All about the Theme Studio and more"), p("theme studio again")],
      "theme studio",
      "/post/theme-studio",
    );
    expect(next).not.toBeNull();
    expect(next?.[0]?.attrs.text).toBe(
      'All about the <a href="/post/theme-studio">Theme Studio</a> and more',
    );
    expect(next?.[1]?.attrs.text).toBe("theme studio again");
  });

  it("reaches nested children and reports a miss as null", () => {
    const group: Block = { kind: "group", attrs: {}, children: [p("deep theme studio here")] };
    const next = linkPhrase([group], "theme studio", "/x");
    expect(next?.[0]?.children[0]?.attrs.text).toContain('<a href="/x">');
    expect(linkPhrase([p("nothing relevant")], "theme studio", "/x")).toBeNull();
  });

  it("never links inside a tag or an existing link", () => {
    const already = p('see <a href="/old">the theme studio</a> here');
    expect(linkPhrase([already], "theme studio", "/new")).toBeNull();
    const inTag = p('<a href="/theme-studio-notes">notes</a> and theme studio');
    const next = linkPhrase([inTag], "theme studio", "/new");
    expect(next?.[0]?.attrs.text).toBe(
      '<a href="/theme-studio-notes">notes</a> and <a href="/new">theme studio</a>',
    );
  });

  it("does not mutate the input blocks", () => {
    const original = [p("the theme studio")];
    linkPhrase(original, "theme studio", "/x");
    expect(original[0]?.attrs.text).toBe("the theme studio");
  });
});

describe("appendFurtherReading", () => {
  it("adds one escaped closing paragraph", () => {
    const next = appendFurtherReading([p("body")], 'A "quoted" <title>', "/post/q");
    expect(next).toHaveLength(2);
    expect(next[1]?.attrs.text).toBe(
      'Related: <a href="/post/q">A &quot;quoted&quot; &lt;title&gt;</a>',
    );
  });
});
