import * as React from "react";
import { useMutation, useQueries, useQuery } from "@tanstack/react-query";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import type { Block } from "./blocks";
import type { ChromeValue } from "./PostChromeSidebar";
import { MediaPicker } from "./MediaPicker";
import { mediaUrl } from "./blocks";
import {
  headingLint,
  imageFindings,
  imageRefs,
  keyphraseReport,
  linkAudit,
  readability,
  SERP_DESC_MAX_PX,
  SERP_TITLE_MAX_PX,
  serpWidths,
  slugQuality,
  type Finding,
} from "./seo";

/**
 * Everything about how this entry will be found: a keyphrase scorecard,
 * readability, the result as Google draws it, the address, links,
 * headings, images, the social card, structured data, robots, and what
 * the rest of the site says about it. Analysis is client-side and live;
 * the two server calls (site signals, link check) are explicit.
 */
export function SeoPanel({
  chrome,
  onChange,
  title,
  blocks,
  postId,
  isLive,
  path,
}: {
  chrome: ChromeValue;
  onChange: (patch: Partial<ChromeValue>) => void;
  title: string;
  blocks: Block[];
  postId?: string;
  isLive: boolean;
  path: string;
}) {
  const description = chrome.seo_description.trim() !== "" ? chrome.seo_description : chrome.excerpt;
  const searchTitle = chrome.seo_title.trim() !== "" ? chrome.seo_title : title;
  const keyphrase = React.useMemo(
    () => keyphraseReport({ keyphrase: chrome.seo_keyphrase, title: searchTitle, slug: chrome.slug, description, blocks }),
    [chrome.seo_keyphrase, searchTitle, chrome.slug, description, blocks],
  );
  const read = React.useMemo(() => readability(blocks), [blocks]);
  const slug = React.useMemo(() => slugQuality(chrome.slug), [chrome.slug]);
  const links = React.useMemo(() => linkAudit(blocks, path), [blocks, path]);
  const headings = React.useMemo(() => headingLint(blocks), [blocks]);
  const widths = serpWidths(searchTitle, description);

  const refs = React.useMemo(() => imageRefs(blocks), [blocks]);
  const mediaQueries = useQueries({
    queries: refs
      .filter((r) => r.mediaId !== null)
      .slice(0, 20)
      .map((r) => ({
        queryKey: ["media", r.mediaId],
        queryFn: () => api.getMedia(r.mediaId as string),
        staleTime: 5 * 60 * 1000,
      })),
  });
  const images = mediaQueries.flatMap((q) => (q.data ? [{ width: q.data.width ?? null, byte_size: Number(q.data.byte_size) }] : []));
  const imageF = imageFindings(images, 1200);

  const signals = useQuery({
    queryKey: ["seo-signals", postId, chrome.seo_keyphrase],
    queryFn: () => api.seoSignals(postId as string),
    enabled: postId !== undefined && isLive,
    staleTime: 60_000,
  });
  const lastCheck = useQuery({
    queryKey: ["link-check", postId],
    queryFn: () => api.lastLinkCheck(postId as string),
    enabled: postId !== undefined,
    retry: false,
  });
  const check = useMutation({
    mutationFn: () => api.checkLinks(postId as string),
    onSuccess: () => void lastCheck.refetch(),
    onError: (e) => notify.error("Couldn't check the links", e),
  });

  const overall = [...keyphrase, ...read.findings, ...slug, ...links.findings, ...headings];
  const bad = overall.filter((f) => f.level === "bad").length;
  const warn = overall.filter((f) => f.level === "warn").length;

  return (
    <div className="space-y-1 text-sm" data-testid="seo-panel">
      <p className="px-1 text-xs text-muted-foreground" data-testid="seo-summary">
        {bad === 0 && warn === 0 ? "Nothing to fix." : `${bad} to fix, ${warn} to consider.`}
      </p>

      <Card title="Focus keyphrase">
        <Input
          value={chrome.seo_keyphrase}
          onChange={(e) => onChange({ seo_keyphrase: e.target.value })}
          placeholder="What should this rank for?"
          aria-label="Focus keyphrase"
          className="h-8"
        />
        <Findings items={keyphrase} testId="keyphrase-findings" />
        {signals.data && signals.data.keyphrase_rivals.length > 0 ? (
          <p className="text-xs text-amber-600 dark:text-amber-400" data-testid="keyphrase-rivals">
            Also targeted by{" "}
            {signals.data.keyphrase_rivals.map((r, i) => (
              <React.Fragment key={r.id}>
                {i > 0 ? ", " : ""}
                <a href={`/admin/posts/${r.id}`} className="underline">{r.title}</a>
              </React.Fragment>
            ))}
            . Two posts on one keyphrase compete with each other.
          </p>
        ) : null}
      </Card>

      <Card title="Readability">
        <Findings items={read.findings} />
      </Card>

      <Card title="In search results">
        <div className="rounded border p-2">
          <p className="truncate text-[11px] text-muted-foreground">{path}</p>
          <p className="truncate text-[15px] text-[#1a0dab] dark:text-[#8ab4f8]" style={{ maxWidth: SERP_TITLE_MAX_PX / 1.25 }}>{searchTitle || "Untitled"}</p>
          <p className="line-clamp-2 text-xs text-muted-foreground">{description || "No description yet."}</p>
        </div>
        <Budget label="Title" px={widths.titlePx} max={SERP_TITLE_MAX_PX} />
        <Budget label="Description" px={widths.descPx} max={SERP_DESC_MAX_PX} />
        <p className="text-[11px] text-muted-foreground">Google cuts by width, not letters: a title of wide letters runs out sooner.</p>
      </Card>

      <Card title="Address">
        <Findings items={slug} />
        {signals.data && signals.data.redirects_here.length > 0 ? (
          <p className="text-xs text-muted-foreground">
            Old addresses redirecting here: {signals.data.redirects_here.join(", ")}.
          </p>
        ) : null}
        {isLive ? (
          <p className="text-[11px] text-muted-foreground">Changing a live address writes a redirect from the old one automatically.</p>
        ) : null}
      </Card>

      <Card title="Links">
        <Findings items={links.findings} />
        {signals.data ? (
          <p className={cn("text-xs", signals.data.inbound_links === 0 ? "text-amber-600 dark:text-amber-400" : "text-muted-foreground")} data-testid="inbound-links">
            {signals.data.inbound_links === 0 ? "No other published entry links here (an orphan)." : `${signals.data.inbound_links} other entr${signals.data.inbound_links === 1 ? "y links" : "ies link"} here.`}
          </p>
        ) : null}
        {postId !== undefined ? (
          <div className="space-y-1">
            <Button type="button" variant="outline" size="sm" disabled={check.isPending} onClick={() => check.mutate()}>
              {check.isPending ? "Checking…" : "Check outbound links"}
            </Button>
            {lastCheck.data ? (
              lastCheck.data.broken.length === 0 ? (
                <p className="text-xs text-muted-foreground">No broken links as of {new Date(lastCheck.data.checked_at).toLocaleString()}.</p>
              ) : (
                <ul className="space-y-0.5 text-xs text-destructive" data-testid="broken-links">
                  {lastCheck.data.broken.map((b) => (
                    <li key={b.url} className="truncate">{b.status === 0 ? "No answer" : b.status}: {b.url}</li>
                  ))}
                </ul>
              )
            ) : null}
            <p className="text-[11px] text-muted-foreground">Published entries are also swept nightly; broken links show on Site health.</p>
          </div>
        ) : null}
      </Card>

      <Card title="Headings">
        <Findings items={headings} />
      </Card>

      {imageF.length > 0 ? (
        <Card title="Images">
          <Findings items={imageF} />
        </Card>
      ) : null}

      <Card title="Social card">
        <Field label="Title for feeds" htmlFor="og-title" hint="Defaults to the search title.">
          <Input id="og-title" value={chrome.og_title} onChange={(e) => onChange({ og_title: e.target.value })} className="h-8" />
        </Field>
        <Field label="Description for feeds" htmlFor="og-desc" hint="Defaults to the search description.">
          <textarea id="og-desc" rows={2} value={chrome.og_description} onChange={(e) => onChange({ og_description: e.target.value })} className="w-full rounded-md border border-input bg-background p-2 text-sm" />
        </Field>
        <div className="space-y-1">
          <p className="text-xs font-medium">Image for feeds</p>
          {chrome.og_image !== "" ? (
            <div className="space-y-1">
              <img src={chrome.og_image} alt="" className="max-h-28 w-full rounded border object-cover" />
              <Button type="button" variant="ghost" size="sm" onClick={() => onChange({ og_image: "" })}>Use the featured image</Button>
            </div>
          ) : (
            <p className="text-xs text-muted-foreground">Defaults to the featured image.</p>
          )}
          <MediaPicker accept="image" onPick={(m) => onChange({ og_image: `${mediaUrl(m.id)}?variant=large` })} />
        </div>
        <SocialCard title={chrome.og_title || searchTitle} description={chrome.og_description || description} image={chrome.og_image || chrome.featured_media_url} />
      </Card>

      <Card title="Structured data">
        <Field label="Type" htmlFor="schema-type" hint="Article is always emitted; this adds a richer type built from the blocks.">
          <select id="schema-type" value={chrome.schema_type} onChange={(e) => onChange({ schema_type: e.target.value })} className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm">
            <option value="">Article only</option>
            <option value="faq">FAQ (from Details blocks)</option>
            <option value="howto">How-to (from the first numbered list)</option>
            <option value="product">Product (price below)</option>
          </select>
        </Field>
        {chrome.schema_type === "product" ? (
          <div className="grid grid-cols-2 gap-2">
            <Input value={chrome.product_price} onChange={(e) => onChange({ product_price: e.target.value })} placeholder="Price, e.g. 12.50" aria-label="Price" className="h-8" />
            <Input value={chrome.product_currency} onChange={(e) => onChange({ product_currency: e.target.value.toUpperCase() })} placeholder="Currency, e.g. USD" aria-label="Currency" className="h-8" maxLength={3} />
          </div>
        ) : null}
        <SchemaCheck type={chrome.schema_type} blocks={blocks} price={chrome.product_price} />
      </Card>

      <Card title="Advanced">
        <Field label="Canonical URL" htmlFor="seo-canonical" hint="Only for content that lives elsewhere first (syndication). Leave empty otherwise.">
          <Input id="seo-canonical" value={chrome.seo_canonical} onChange={(e) => onChange({ seo_canonical: e.target.value })} placeholder="https://…" className="h-8 font-mono text-xs" />
        </Field>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={chrome.seo_noindex} onChange={(e) => onChange({ seo_noindex: e.target.checked })} className="accent-primary" />
          Keep out of search engines (noindex)
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" checked={chrome.seo_nofollow} onChange={(e) => onChange({ seo_nofollow: e.target.checked })} className="accent-primary" />
          Don't pass link value from this page (nofollow)
        </label>
      </Card>
    </div>
  );
}

function Card({ title, children }: { title: string; children: React.ReactNode }) {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="rounded-md border">
      <button type="button" onClick={() => setOpen((v) => !v)} aria-expanded={open} className="flex w-full items-center px-3 py-2 text-left text-xs font-medium hover:bg-muted/50">
        {title}
        <span className="ml-auto text-muted-foreground">{open ? "−" : "+"}</span>
      </button>
      {open ? <div className="space-y-2 px-3 pb-3">{children}</div> : null}
    </div>
  );
}

function Findings({ items, testId }: { items: Finding[]; testId?: string }) {
  return (
    <ul className="space-y-1" data-testid={testId}>
      {items.map((f) => (
        <li key={f.text} className="flex items-start gap-2 text-xs">
          <span aria-hidden="true" className={cn("mt-1 h-2 w-2 shrink-0 rounded-full", f.level === "ok" ? "bg-success" : f.level === "warn" ? "bg-warning" : "bg-destructive")} />
          <span className={f.level === "ok" ? "text-muted-foreground" : ""}>{f.text}</span>
        </li>
      ))}
    </ul>
  );
}

function Budget({ label, px, max }: { label: string; px: number; max: number }) {
  const over = px > max;
  return (
    <p className={cn("text-xs", over ? "text-destructive" : "text-muted-foreground")}>
      {label}: {px} of {max}px{over ? " — will be cut off" : ""}
    </p>
  );
}

function SocialCard({ title, description, image }: { title: string; description: string; image: string }) {
  return (
    <div className="overflow-hidden rounded border" data-testid="social-card">
      {image !== "" ? <img src={image} alt="" className="max-h-32 w-full object-cover" /> : <div className="flex h-16 items-center justify-center bg-muted text-xs text-muted-foreground">No image</div>}
      <div className="border-t p-2">
        <p className="truncate text-sm font-medium">{title || "Untitled"}</p>
        <p className="line-clamp-2 text-xs text-muted-foreground">{description}</p>
      </div>
    </div>
  );
}

/** Says whether the chosen type has what it needs, before it is published. */
function SchemaCheck({ type, blocks, price }: { type: string; blocks: Block[]; price: string }) {
  if (type === "") return null;
  let ok = true;
  let text = "";
  if (type === "faq") {
    const n = countKind(blocks, "details");
    ok = n > 0;
    text = ok ? `${n} question${n === 1 ? "" : "s"} from Details blocks.` : "Add Details blocks: the summary is the question, the content is the answer.";
  } else if (type === "howto") {
    const has = blocks.some((b) => b.kind === "list" && b.attrs["ordered"] === true);
    ok = has;
    text = has ? "Steps come from the first numbered list." : "Add a numbered list; each item becomes a step.";
  } else if (type === "product") {
    ok = /^\d+(\.\d{1,2})?$/.test(price.trim());
    text = ok ? "Product with an offer." : "Enter a price like 12.50.";
  }
  return <p className={cn("text-xs", ok ? "text-muted-foreground" : "text-amber-600 dark:text-amber-400")} data-testid="schema-check">{text}</p>;
}

function countKind(blocks: Block[], kind: string): number {
  let n = 0;
  const visit = (list: Block[]) => {
    for (const b of list ?? []) {
      if (b.kind === kind) n += 1;
      visit(b.children ?? []);
    }
  };
  visit(blocks);
  return n;
}
