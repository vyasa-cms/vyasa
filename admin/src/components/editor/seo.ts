/**
 * SEO analysis, all client-side and pure: keyphrase use, readability,
 * slug quality, link and heading audits, SERP pixel widths. Every
 * function takes the document and returns findings; the panel renders
 * them. Nothing here talks to the server.
 */
import { stripTags, type Block } from "./blocks";

export interface Finding {
  level: "ok" | "warn" | "bad";
  text: string;
}

/** Text-bearing attributes, in document order, with their block kind. */
function textRuns(blocks: Block[]): { kind: string; text: string; level?: number }[] {
  const out: { kind: string; text: string; level?: number }[] = [];
  const visit = (list: Block[]) => {
    for (const b of list ?? []) {
      for (const key of ["text", "caption", "summary", "title"]) {
        const v = b.attrs[key];
        if (typeof v === "string" && v.trim() !== "") {
          out.push({
            kind: b.kind,
            text: stripTags(v),
            ...(b.kind === "heading" ? { level: Number(b.attrs["level"] ?? 2) } : {}),
          });
        }
      }
      visit(b.children ?? []);
    }
  };
  visit(blocks);
  return out;
}

const norm = (s: string) => s.toLowerCase().replace(/\s+/g, " ").trim();
const words = (s: string) => s.split(/\s+/).filter((w) => w !== "");

/* -------------------------------------------------------------- keyphrase */

export function keyphraseReport(input: {
  keyphrase: string;
  title: string;
  slug: string;
  description: string;
  blocks: Block[];
}): Finding[] {
  const kp = norm(input.keyphrase);
  if (kp === "") return [{ level: "warn", text: "Set a focus keyphrase to get a scorecard." }];
  const runs = textRuns(input.blocks);
  const body = runs.map((r) => r.text).join("\n");
  const has = (s: string) => norm(s).includes(kp);
  const findings: Finding[] = [];
  findings.push(has(input.title) ? { level: "ok", text: "Keyphrase is in the title." } : { level: "bad", text: "Keyphrase is not in the title." });
  const firstPara = runs.find((r) => r.kind === "paragraph")?.text ?? "";
  findings.push(has(firstPara) ? { level: "ok", text: "Keyphrase appears in the opening paragraph." } : { level: "warn", text: "Keyphrase is not in the opening paragraph." });
  const inHeading = runs.some((r) => r.kind === "heading" && has(r.text));
  findings.push(inHeading ? { level: "ok", text: "A heading uses the keyphrase." } : { level: "warn", text: "No heading uses the keyphrase." });
  const slugWords = kp.replace(/[^a-z0-9 ]/g, "").split(" ").filter(Boolean);
  const slugHas = slugWords.length > 0 && slugWords.every((w) => input.slug.toLowerCase().includes(w));
  findings.push(slugHas ? { level: "ok", text: "Keyphrase is in the address." } : { level: "warn", text: "Keyphrase is not in the address." });
  findings.push(has(input.description) ? { level: "ok", text: "Keyphrase is in the search description." } : { level: "warn", text: "Keyphrase is not in the search description." });
  const alts: string[] = [];
  const visit = (list: Block[]) => {
    for (const b of list ?? []) {
      if (typeof b.attrs["alt"] === "string") alts.push(b.attrs["alt"] as string);
      visit(b.children ?? []);
    }
  };
  visit(input.blocks);
  if (alts.length > 0) {
    findings.push(alts.some(has) ? { level: "ok", text: "An image's alt text uses the keyphrase." } : { level: "warn", text: "No image alt text uses the keyphrase." });
  }
  const total = words(body).length;
  const count = total === 0 ? 0 : norm(body).split(kp).length - 1;
  const density = total === 0 ? 0 : (count * words(kp).length * 100) / total;
  if (total < 50) findings.push({ level: "warn", text: "Too little text to judge keyphrase density." });
  else if (count === 0) findings.push({ level: "bad", text: "The keyphrase never appears in the text." });
  else if (density > 3) findings.push({ level: "warn", text: `Keyphrase density is ${density.toFixed(1)}%; over 3% reads as stuffing.` });
  else findings.push({ level: "ok", text: `Keyphrase appears ${count} time${count === 1 ? "" : "s"} (${density.toFixed(1)}%).` });
  return findings;
}

/* ------------------------------------------------------------ readability */

const PASSIVE = /\b(am|is|are|was|were|be|been|being)\s+(\w+ed|\w+en|built|done|made|put|set|sent|shown|taken|given|known|seen|written|held|kept|left|lost|paid|read|said|sold|told|thought|won|found)\b/gi;

function syllables(word: string): number {
  const w = word.toLowerCase().replace(/[^a-z]/g, "");
  if (w.length <= 3) return 1;
  const m = w.replace(/(?:[^laeiouy]es|ed|[^laeiouy]e)$/, "").replace(/^y/, "").match(/[aeiouy]{1,2}/g);
  return Math.max(1, m?.length ?? 1);
}

export interface Readability {
  flesch: number;
  sentences: number;
  longSentences: number;
  longParagraphs: number;
  passive: number;
  wordsPerHeading: number | null;
  findings: Finding[];
}

export function readability(blocks: Block[]): Readability {
  const runs = textRuns(blocks);
  const paras = runs.filter((r) => r.kind === "paragraph").map((r) => r.text);
  const text = paras.join(" ");
  const sentenceList = text.split(/(?<=[.!?])\s+/).map((s) => s.trim()).filter((s) => words(s).length > 0);
  const wordList = words(text);
  const syl = wordList.reduce((n, w) => n + syllables(w), 0);
  const flesch =
    wordList.length === 0 || sentenceList.length === 0
      ? 0
      : Math.round(206.835 - 1.015 * (wordList.length / sentenceList.length) - 84.6 * (syl / wordList.length));
  const longSentences = sentenceList.filter((s) => words(s).length > 25).length;
  const longParagraphs = paras.filter((p) => words(p).length > 150).length;
  const passive = (text.match(PASSIVE) ?? []).length;
  const headings = runs.filter((r) => r.kind === "heading").length;
  const wordsPerHeading = wordList.length >= 300 ? Math.round(wordList.length / Math.max(1, headings)) : null;
  const findings: Finding[] = [];
  if (wordList.length < 50) {
    findings.push({ level: "warn", text: "Not enough text to score yet." });
    return { flesch, sentences: sentenceList.length, longSentences, longParagraphs, passive, wordsPerHeading, findings };
  }
  findings.push(
    flesch >= 60 ? { level: "ok", text: `Reading ease ${flesch}: plain and easy.` }
    : flesch >= 40 ? { level: "warn", text: `Reading ease ${flesch}: fairly dense.` }
    : { level: "bad", text: `Reading ease ${flesch}: hard going; shorter sentences and words help.` },
  );
  const longShare = sentenceList.length === 0 ? 0 : longSentences / sentenceList.length;
  findings.push(longShare <= 0.25 ? { level: "ok", text: `${longSentences} of ${sentenceList.length} sentences run past 25 words.` } : { level: "warn", text: `${longSentences} of ${sentenceList.length} sentences run past 25 words; split some.` });
  findings.push(longParagraphs === 0 ? { level: "ok", text: "No paragraph is over 150 words." } : { level: "warn", text: `${longParagraphs} paragraph${longParagraphs === 1 ? "" : "s"} over 150 words.` });
  const passiveShare = sentenceList.length === 0 ? 0 : passive / sentenceList.length;
  findings.push(passiveShare <= 0.1 ? { level: "ok", text: "Little passive voice." } : { level: "warn", text: `About ${passive} passive sentence${passive === 1 ? "" : "s"}; prefer the active voice.` });
  if (wordsPerHeading !== null) {
    findings.push(headings > 0 && wordsPerHeading <= 300 ? { level: "ok", text: `A heading every ~${wordsPerHeading} words.` } : { level: "warn", text: headings === 0 ? "No headings in a long post; add some." : `Only a heading every ~${wordsPerHeading} words; more would help skimming.` });
  }
  return { flesch, sentences: sentenceList.length, longSentences, longParagraphs, passive, wordsPerHeading, findings };
}

/* ------------------------------------------------------------------- slug */

const STOP = new Set(["a", "an", "the", "and", "or", "of", "to", "in", "on", "for", "with", "is", "at", "by", "from", "that", "this"]);

export function slugQuality(slug: string): Finding[] {
  if (slug.trim() === "") return [{ level: "warn", text: "The address is made from the title on save." }];
  const parts = slug.split("-").filter(Boolean);
  const findings: Finding[] = [];
  if (slug.length > 75) findings.push({ level: "warn", text: `${slug.length} characters; keep addresses under 75.` });
  const stops = parts.filter((p) => STOP.has(p));
  if (stops.length > 0) findings.push({ level: "warn", text: `Stop words in the address: ${stops.join(", ")}.` });
  if (/^\d+$/.test(parts[0] ?? "")) findings.push({ level: "warn", text: "The address starts with a number." });
  if (/[A-Z_\s]/.test(slug)) findings.push({ level: "bad", text: "Use lowercase letters and dashes only." });
  if (findings.length === 0) findings.push({ level: "ok", text: "Short, lowercase, no filler." });
  return findings;
}

/* ------------------------------------------------------------------ links */

export interface LinkAudit {
  internal: number;
  external: number;
  findings: Finding[];
}

const VAGUE = new Set(["click here", "here", "read more", "this", "link", "more", "this link"]);

export function linkAudit(blocks: Block[], selfPath: string): LinkAudit {
  const links: { href: string; text: string }[] = [];
  const visit = (list: Block[]) => {
    for (const b of list ?? []) {
      for (const key of ["text", "caption", "summary", "title"]) {
        const v = b.attrs[key];
        if (typeof v !== "string") continue;
        for (const m of v.matchAll(/<a\s[^>]*href="([^"]*)"[^>]*>(.*?)<\/a>/gi)) {
          links.push({ href: (m[1] ?? "").replace(/&amp;/g, "&"), text: stripTags(m[2] ?? "").trim() });
        }
      }
      if (b.kind === "button" && typeof b.attrs["href"] === "string") {
        links.push({ href: b.attrs["href"] as string, text: String(b.attrs["label"] ?? "") });
      }
      visit(b.children ?? []);
    }
  };
  visit(blocks);
  const isExternal = (h: string) => /^https?:\/\//i.test(h) && (typeof window === "undefined" || !h.startsWith(window.location.origin));
  const internal = links.filter((l) => !isExternal(l.href) && !l.href.startsWith("#") && !l.href.startsWith("mailto:")).length;
  const external = links.filter((l) => isExternal(l.href)).length;
  const findings: Finding[] = [];
  const vague = links.filter((l) => VAGUE.has(l.text.toLowerCase()));
  if (vague.length > 0) findings.push({ level: "warn", text: `${vague.length} link${vague.length === 1 ? "" : "s"} with vague text ("${vague[0]?.text}"); say where it goes.` });
  const counts = new Map<string, number>();
  for (const l of links) counts.set(l.href, (counts.get(l.href) ?? 0) + 1);
  const dup = [...counts.entries()].filter(([, n]) => n > 1);
  if (dup.length > 0) findings.push({ level: "warn", text: `${dup[0]?.[0]} is linked ${dup[0]?.[1]} times.` });
  if (links.some((l) => l.href === selfPath)) findings.push({ level: "warn", text: "The post links to itself." });
  if (internal === 0 && links.length + 1 > 0) findings.push({ level: "warn", text: "No internal links; link to a related post or page." });
  else findings.push({ level: "ok", text: `${internal} internal, ${external} external link${external === 1 ? "" : "s"}.` });
  return { internal, external, findings };
}

/* --------------------------------------------------------------- headings */

export function headingLint(blocks: Block[]): Finding[] {
  const hs = textRuns(blocks).filter((r) => r.kind === "heading");
  const findings: Finding[] = [];
  let prev = 1;
  for (const h of hs) {
    const level = h.level ?? 2;
    if (level > prev + 1) findings.push({ level: "warn", text: `"${h.text.slice(0, 40)}" jumps from H${prev} to H${level}.` });
    if (h.text.trim() === "") findings.push({ level: "bad", text: "An empty heading." });
    prev = level;
  }
  const htmlH1 = blocks.some((b) => b.kind === "html" && /<h1[\s>]/i.test(String(b.attrs["html"] ?? "")));
  if (htmlH1) findings.push({ level: "bad", text: "A Custom HTML block contains an H1; the title is the page's H1." });
  if (findings.length === 0) findings.push({ level: "ok", text: hs.length === 0 ? "No headings yet." : "Heading levels are in order." });
  return findings;
}

/* ------------------------------------------------------------- SERP pixels */

/** Google's result typography: title 20px, description 14px, Arial. */
export const SERP_TITLE_MAX_PX = 600;
export const SERP_DESC_MAX_PX = 990;

let canvas: HTMLCanvasElement | null = null;
export function textWidthPx(text: string, font: string): number {
  if (typeof document === "undefined") return text.length * 8;
  canvas ??= document.createElement("canvas");
  const ctx = ((): CanvasRenderingContext2D | null => {
    try {
      return canvas?.getContext("2d") ?? null;
    } catch {
      return null;
    }
  })();
  if (ctx === null) return text.length * (font.startsWith("20") ? 9.5 : 6.6);
  ctx.font = font;
  const w = ctx.measureText(text).width;
  // jsdom measures nothing; keep a per-character estimate so budgets still show.
  return w > 0 ? w : text.length * (font.startsWith("20") ? 9.5 : 6.6);
}

export function serpWidths(title: string, description: string): { titlePx: number; descPx: number } {
  return {
    titlePx: Math.round(textWidthPx(title, "20px Arial")),
    descPx: Math.round(textWidthPx(description, "14px Arial")),
  };
}

/* ------------------------------------------------------------ image weight */

export interface ImageRef {
  mediaId: string | null;
  url: string;
}

/** Library images in the document, with their media ids where known. */
export function imageRefs(blocks: Block[]): ImageRef[] {
  const out: ImageRef[] = [];
  const visit = (list: Block[]) => {
    for (const b of list ?? []) {
      const url = b.attrs["url"];
      if ((b.kind === "image" || b.kind === "cover" || b.kind === "media_text") && typeof url === "string" && url !== "") {
        const m = /\/api\/v1\/media\/(\d+)\/raw/.exec(url);
        out.push({ mediaId: m?.[1] ?? null, url });
      }
      visit(b.children ?? []);
    }
  };
  visit(blocks);
  return out;
}

export function imageFindings(images: { width: number | null; byte_size: number }[], contentWidth: number): Finding[] {
  const total = images.reduce((n, i) => n + i.byte_size, 0);
  const findings: Finding[] = [];
  if (images.length === 0) return findings;
  const mb = total / (1024 * 1024);
  findings.push(mb <= 1.5 ? { level: "ok", text: `${images.length} image${images.length === 1 ? "" : "s"}, ${mb.toFixed(1)} MB in originals; readers get resized copies.` } : { level: "warn", text: `${images.length} images, ${mb.toFixed(1)} MB in originals.` });
  const wide = images.filter((i) => i.width !== null && i.width > contentWidth * 2).length;
  if (wide > 0) findings.push({ level: "warn", text: `${wide} image${wide === 1 ? " is" : "s are"} more than twice the content width; fine, but resized copies are what readers see.` });
  return findings;
}
