import { describe, expect, it } from "vitest";
import { htmlToInlineMarkdown, inlineMarkdownToHtml } from "@/components/editor/inlineMarkdown";

describe("classic inline markdown", () => {
  it("shows stored HTML as readable markdown", () => {
    expect(htmlToInlineMarkdown('Say <strong>hi</strong> to <a href="/about">us</a><br>now')).toBe(
      "Say **hi** to [us](/about)\nnow",
    );
  });

  it("writes the same tags the canvas writes", () => {
    expect(inlineMarkdownToHtml("**b** *i* `c` ~~s~~ [t](/u)\nx")).toBe(
      "<strong>b</strong> <em>i</em> <code>c</code> <s>s</s> <a href=\"/u\">t</a><br>x",
    );
  });

  it("round-trips text that looks like syntax without changing it", () => {
    const html = "2 * 3 = 6, a &lt; b, ~tilde~, [not a link], `tick";
    const md = htmlToInlineMarkdown(html);
    expect(md).toBe("2 \\* 3 = 6, a \\< b, \\~tilde\\~, \\[not a link], \\`tick");
    expect(inlineMarkdownToHtml(md)).toBe(html);
  });

  it("keeps the marks this syntax has no spelling for", () => {
    const html = "H<sub>2</sub>O and <mark>this</mark>";
    const md = htmlToInlineMarkdown(html);
    expect(md).toBe(html);
    expect(inlineMarkdownToHtml(md)).toBe(html);
  });

  it("escapes anything else that could be markup", () => {
    expect(inlineMarkdownToHtml("<script>x</script> & <b>y</b>")).toBe(
      "&lt;script&gt;x&lt;/script&gt; &amp; &lt;b&gt;y&lt;/b&gt;",
    );
  });

  it("closes marks left open and nests the ones inside a link", () => {
    expect(inlineMarkdownToHtml("**open")).toBe("<strong>open</strong>");
    expect(inlineMarkdownToHtml("[**bold** link](https://x.y/?a=1)")).toBe(
      '<a href="https://x.y/?a=1"><strong>bold</strong> link</a>',
    );
  });

  it("survives a second round trip unchanged", () => {
    const html = '<em>a <strong>b</strong></em> <code>x*y</code> <a href="/p">q</a>';
    const once = inlineMarkdownToHtml(htmlToInlineMarkdown(html));
    expect(inlineMarkdownToHtml(htmlToInlineMarkdown(once))).toBe(once);
  });
});
