import { describe, expect, it } from "vitest";
import { headingLint, keyphraseReport, linkAudit, readability, slugQuality, serpWidths } from "@/components/editor/seo";

const p = (text: string) => ({ kind: "paragraph", attrs: { text }, children: [] });
const h = (level: number, text: string) => ({ kind: "heading", attrs: { level, text }, children: [] });

describe("keyphrase scorecard", () => {
  it("checks title, opening, heading, slug, description and density", () => {
    const blocks = [p("Sourdough starter care is simple. " + "More words here. ".repeat(60)), h(2, "Sourdough starter care daily")] as never;
    const r = keyphraseReport({ keyphrase: "sourdough starter", title: "Sourdough starter care", slug: "sourdough-starter-care", description: "How to keep a sourdough starter alive", blocks });
    const levels = r.map((f) => f.level);
    expect(levels.filter((l) => l === "bad")).toHaveLength(0);
    expect(r.at(-1)?.text).toMatch(/appears 2 times/);
  });
  it("flags a keyphrase missing from the title and text", () => {
    const r = keyphraseReport({ keyphrase: "quantum", title: "Bread", slug: "bread", description: "", blocks: [p("Words. ".repeat(60))] as never });
    expect(r[0]).toEqual({ level: "bad", text: "Keyphrase is not in the title." });
    expect(r.some((f) => f.text === "The keyphrase never appears in the text.")).toBe(true);
  });
});

describe("readability", () => {
  it("scores plain prose as easy and dense prose as hard", () => {
    const easy = readability([p("The cat sat on the mat. It was warm. The sun was out. ".repeat(6))] as never);
    expect(easy.flesch).toBeGreaterThan(80);
    const dense = readability([p("Notwithstanding the aforementioned considerations regarding organisational restructuring across the enterprise, the implementation methodology necessitates comprehensive stakeholder consultation procedures before any deliverable is formally accepted by the governance committee. ".repeat(6))] as never);
    expect(dense.flesch).toBeLessThan(30);
    expect(dense.longSentences).toBeGreaterThan(0);
  });
  it("counts passive sentences", () => {
    const r = readability([p("The report was written by the team. Mistakes were made. " + "We fix them. ".repeat(20))] as never);
    expect(r.passive).toBe(2);
  });
});

describe("slug quality", () => {
  it("flags stop words, length and case", () => {
    expect(slugQuality("the-best-of-the-things").some((f) => f.text.includes("Stop words"))).toBe(true);
    expect(slugQuality("Hello_World")[0]?.level).toBe("bad");
    expect(slugQuality("clean-address")).toEqual([{ level: "ok", text: "Short, lowercase, no filler." }]);
  });
});

describe("link audit", () => {
  it("flags vague text, repeats, self links and no internal links", () => {
    const blocks = [p('See <a href="https://x.example/">click here</a> and <a href="https://x.example/">again</a> or <a href="/post/me">me</a>')] as never;
    const r = linkAudit(blocks, "/post/me");
    const texts = r.findings.map((f) => f.text);
    expect(texts.some((t) => t.includes("vague text"))).toBe(true);
    expect(texts.some((t) => t.includes("linked 2 times"))).toBe(true);
    expect(texts.some((t) => t.includes("links to itself"))).toBe(true);
    expect(r.external).toBe(2);
    expect(r.internal).toBe(1);
  });
});

describe("heading lint", () => {
  it("catches skipped levels and an H1 in raw HTML", () => {
    const r = headingLint([h(2, "A"), h(4, "B"), { kind: "html", attrs: { html: "<h1>x</h1>" }, children: [] }] as never);
    expect(r.map((f) => f.text)).toEqual(['"B" jumps from H2 to H4.', "A Custom HTML block contains an H1; the title is the page's H1."]);
  });
});

describe("serp widths", () => {
  it("measures wider titles as wider", () => {
    const a = serpWidths("Short", "d");
    const b = serpWidths("A much longer title that keeps going and going", "d");
    expect(b.titlePx).toBeGreaterThan(a.titlePx);
  });
});
