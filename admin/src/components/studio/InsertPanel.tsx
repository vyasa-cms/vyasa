import * as React from "react";

import type { Vocabulary } from "@/api/themes";
import { INSERT_MIME } from "./useCanvasBridge";
import { cn } from "@/lib/utils";

/**
 * The insert library: every kind the registry serves, as a card with a
 * small wireframe of what it puts on the page.
 *
 * Grouping and starter settings come from the vocabulary, so a kind a
 * plugin registers later lands here with no admin change — the library is
 * a view of the registry, not a hand-kept list.
 */

const CATEGORY_ORDER = [
  "structure",
  "marketing",
  "media",
  "content",
  "commerce",
  "community",
  "navigation",
  "chrome",
] as const;
const CATEGORY_LABELS: Record<string, string> = {
  structure: "Structure",
  marketing: "Marketing",
  media: "Media",
  content: "Content",
  commerce: "Commerce",
  community: "Community",
  navigation: "Navigation",
  chrome: "Chrome",
};

/** One entry the library can insert. */
export interface InsertableKind {
  kind: string;
  description: string;
  category: string;
  /** Owning plugin, when a plugin registered the kind. */
  plugin?: string;
}

/**
 * Flattens the vocabulary into insertable entries.
 *
 * Static regions (header, footer, …) are chrome: they come without a
 * category of their own, and a kind the server knows but nothing claims
 * still has to be reachable, so the fallback bucket is `content`.
 */
export function insertableKinds(
  vocab: Vocabulary,
  exclude: readonly string[] = [],
): InsertableKind[] {
  const blocks = vocab.blocks
    .filter((b) => !exclude.includes(b.kind))
    .map((b) => ({
      kind: b.kind,
      description: b.description,
      category: b.category ?? "content",
      plugin: b.plugin,
    }));
  const regions = vocab.static_regions
    .filter((r) => !exclude.includes(r.kind))
    .map((r) => ({ kind: r.kind, description: r.description, category: "chrome" }));
  return [...blocks, ...regions];
}

/** A 56×36 wireframe of what a kind renders. */
function Thumb({ kind }: { kind: string }) {
  const bars = (rows: [number, number, number, number][]) =>
    rows.map(([x, y, w, h], i) => (
      <rect key={i} x={x} y={y} width={w} height={h} rx="1" className="fill-current" />
    ));
  const art: Record<string, React.ReactNode> = {
    band: <rect x="2" y="12" width="52" height="12" rx="1" className="fill-current opacity-80" />,
    columns: bars([
      [4, 8, 22, 20],
      [30, 8, 22, 20],
    ]),
    grid: bars([
      [4, 6, 15, 11],
      [21, 6, 15, 11],
      [38, 6, 15, 11],
      [4, 19, 15, 11],
      [21, 19, 15, 11],
      [38, 19, 15, 11],
    ]),
    group: bars([[8, 8, 40, 20]]),
    hero: (
      <>
        {bars([
          [4, 8, 20, 4],
          [4, 15, 26, 3],
          [4, 22, 10, 5],
        ])}
        <rect x="34" y="6" width="18" height="24" rx="1" className="fill-current opacity-40" />
      </>
    ),
    "feature-grid": bars([
      [5, 8, 6, 6],
      [24, 8, 6, 6],
      [43, 8, 6, 6],
      [5, 18, 12, 3],
      [24, 18, 12, 3],
      [43, 18, 8, 3],
      [5, 24, 10, 2],
      [24, 24, 10, 2],
      [43, 24, 10, 2],
    ]),
    "stats-band": bars([
      [7, 10, 10, 8],
      [24, 10, 10, 8],
      [41, 10, 10, 8],
      [7, 22, 8, 2],
      [24, 22, 8, 2],
      [41, 22, 8, 2],
    ]),
    "cta-band": (
      <>
        <rect x="2" y="4" width="52" height="28" rx="1" className="fill-current opacity-25" />
        {bars([
          [16, 11, 24, 4],
          [23, 20, 10, 5],
        ])}
      </>
    ),
    faq: bars([
      [6, 7, 44, 4],
      [6, 15, 44, 4],
      [6, 23, 44, 4],
    ]),
    "logo-wall": bars([
      [5, 15, 9, 5],
      [17, 15, 9, 5],
      [29, 15, 9, 5],
      [41, 15, 9, 5],
    ]),
    "latest-posts": bars([
      [4, 6, 14, 12],
      [21, 6, 14, 12],
      [38, 6, 14, 12],
      [4, 21, 11, 2],
      [21, 21, 11, 2],
      [38, 21, 11, 2],
    ]),
    "post-content": bars([
      [8, 6, 40, 3],
      [8, 12, 40, 3],
      [8, 18, 40, 3],
      [8, 24, 26, 3],
    ]),
    comments: bars([
      [6, 6, 32, 6],
      [14, 15, 32, 6],
      [6, 24, 32, 6],
    ]),
    "search-box": (
      <>
        <rect
          x="6"
          y="13"
          width="36"
          height="10"
          rx="2"
          className="fill-none stroke-current"
          strokeWidth="1.5"
        />
        {bars([[45, 13, 6, 10]])}
      </>
    ),
    menu: bars([
      [4, 15, 10, 4],
      [17, 15, 10, 4],
      [30, 15, 10, 4],
      [44, 14, 8, 6],
    ]),
    header: bars([
      [4, 14, 14, 6],
      [30, 15, 6, 3],
      [39, 15, 6, 3],
      [48, 15, 5, 3],
    ]),
    footer: bars([
      [4, 10, 12, 3],
      [22, 10, 10, 2],
      [22, 15, 10, 2],
      [38, 10, 12, 8],
      [4, 24, 48, 2],
    ]),
    image: (
      <>
        <rect x="8" y="6" width="40" height="24" rx="2" className="fill-current opacity-25" />
        <path d="M12 26l9-9 7 6 6-5 10 8" className="fill-none stroke-current" strokeWidth="2" />
        <circle cx="20" cy="12" r="2.5" className="fill-current" />
      </>
    ),
    video: (
      <>
        <rect x="8" y="6" width="40" height="24" rx="2" className="fill-current opacity-25" />
        <path d="M24 12l10 6-10 6z" className="fill-current" />
      </>
    ),
    breadcrumbs: bars([
      [4, 16, 8, 3],
      [15, 16, 8, 3],
      [26, 16, 12, 3],
    ]),
    pagination: bars([
      [16, 15, 6, 6],
      [25, 15, 6, 6],
      [34, 15, 6, 6],
    ]),
  };
  return (
    <svg
      viewBox="0 0 56 36"
      aria-hidden="true"
      className="h-12 w-full text-muted-foreground/70"
    >
      {art[kind] ?? bars([[8, 12, 40, 12]])}
    </svg>
  );
}

export function InsertPanel({
  vocab,
  exclude = [],
  onInsert,
}: {
  vocab: Vocabulary;
  exclude?: readonly string[];
  onInsert: (kind: string) => void;
}) {
  const [category, setCategory] = React.useState<string>("all");
  const all = insertableKinds(vocab, exclude);
  const present = CATEGORY_ORDER.filter((c) => all.some((k) => k.category === c));
  const shown = category === "all" ? all : all.filter((k) => k.category === category);

  return (
    <div className="space-y-3" data-testid="insert-panel">
      <div role="tablist" aria-label="Category" className="flex flex-wrap gap-1">
        {["all", ...present].map((c) => (
          <button
            key={c}
            role="tab"
            aria-selected={category === c}
            onClick={() => setCategory(c)}
            className={cn(
              "rounded-full border px-2 py-0.5 text-[11px]",
              category === c
                ? "border-primary/60 bg-primary/10 font-medium text-foreground"
                : "border-border text-muted-foreground hover:text-foreground",
            )}
          >
            {c === "all" ? "All" : CATEGORY_LABELS[c]}
          </button>
        ))}
      </div>

      <div className="grid grid-cols-2 gap-2">
        {shown.map((k) => (
          <button
            key={k.kind}
            type="button"
            aria-label={k.kind}
            title={k.description}
            onClick={() => onInsert(k.kind)}
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData(INSERT_MIME, k.kind);
              e.dataTransfer.effectAllowed = "copy";
            }}
            className="group flex cursor-grab flex-col overflow-hidden rounded-md border bg-card text-left transition-colors hover:border-primary/50 active:cursor-grabbing"
          >
            <span className="border-b bg-muted/40 px-2 pt-2">
              <Thumb kind={k.kind} />
            </span>
            <span className="flex min-w-0 items-center gap-1 px-2 py-1.5">
              <span className="truncate text-[11px] font-medium">{k.kind}</span>
              {k.plugin !== undefined ? (
                <span
                  title={`From the ${k.plugin} plugin`}
                  className="ml-auto shrink-0 rounded bg-primary/10 px-1 py-0.5 text-[8px] font-semibold uppercase tracking-wider text-primary"
                >
                  Plugin
                </span>
              ) : null}
            </span>
          </button>
        ))}
      </div>

      <p className="rounded-md border border-dashed px-2 py-1.5 text-[10px] leading-relaxed text-muted-foreground">
        Click to insert after the selection — or drag a card onto the page
        and drop it exactly where it goes. Double-click text on the page to
        edit it in place.
      </p>
    </div>
  );
}
