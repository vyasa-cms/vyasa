import { describe, expect, it } from "vitest";
import { looksLikeMarkdown, markdownToHtml } from "@/components/editor/markdownPaste";
import { publishChecklist } from "@/components/editor/PostEditor";

describe("markdown paste", () => {
  it("tells markdown from prose", () => {
    expect(looksLikeMarkdown("Just a sentence.")).toBe(false);
    expect(looksLikeMarkdown("# Title\n\nSome **bold** text")).toBe(true);
    expect(looksLikeMarkdown("- one\n- two")).toBe(true);
    expect(looksLikeMarkdown("line one\nline two")).toBe(false);
  });

  it("converts headings, lists, quotes, fences and rules into blocks", () => {
    const html = markdownToHtml(
      "# Top\n\nIntro with *em*.\n\n## Part\n\n- a\n  - a1\n- b\n\n1. first\n\n> quoted\n\n```rust\nfn main() {}\n```\n\n---\n",
    );
    expect(html).toContain("<h2>Top</h2>");
    expect(html).toContain("<p>Intro with <em>em</em>.</p>");
    expect(html).toContain("<h2>Part</h2>");
    expect(html).toContain("<ul><li><p>a</p><ul><li><p>a1</p></li></ul></li><li><p>b</p></li></ul>");
    expect(html).toContain("<ol><li><p>first</p></li></ol>");
    expect(html).toContain("<blockquote><p>quoted</p></blockquote>");
    expect(html).toContain('<pre><code class="language-rust">fn main() {}</code></pre>');
    expect(html).toContain("<hr>");
  });

  it("escapes markup inside code and never emits a stray tag", () => {
    const html = markdownToHtml("```\n<script>x</script>\n```\n\na <b> c");
    expect(html).toContain("&lt;script&gt;x&lt;/script&gt;");
    expect(html).not.toContain("<script>");
    expect(html).toContain("<p>a &lt;b&gt; c</p>");
  });
});

describe("publish checklist", () => {
  it("lists what a reader-facing post is missing", () => {
    const issues = publishChecklist({
      title: "Untitled",
      blocks: [{ kind: "image", attrs: { url: "/a.png", alt: "" }, children: [] }] as never,
      excerpt: "",
      slug: "untitled-2",
      termCount: 0,
      isPage: false,
    });
    expect(issues).toEqual([
      "No title yet.",
      "The post has no text.",
      "1 image has no alt text.",
      "No excerpt; listings will use the opening lines.",
      'The address is "/untitled-2".',
      "No category or tag.",
    ]);
  });

  it("is empty for a finished post, and pages need no category", () => {
    const issues = publishChecklist({
      title: "A real title",
      blocks: [{ kind: "paragraph", attrs: { text: "Words here" }, children: [] }] as never,
      excerpt: "A summary",
      slug: "a-real-title",
      termCount: 0,
      isPage: true,
    });
    expect(issues).toEqual([]);
  });
});
