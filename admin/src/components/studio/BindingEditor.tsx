import type { ContentSource } from "@/api/themes";

const inputClass =
  "h-7 w-full rounded-md border bg-background px-2 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

const SORTS = ["newest", "oldest", "title", "updated"] as const;

/** The shape a `bind` settings object carries. */
export interface BindValue {
  source?: string;
  term?: string;
  sort?: string;
  limit?: number;
}

/**
 * Edits a section's `bind` object: which content the section draws from.
 *
 * Sources come from the vocabulary, so a plugin's type appears here the
 * moment its plugin is enabled and disappears while it is off — the
 * editor never keeps its own list.
 */
export function BindingEditor({
  value,
  sources,
  onChange,
}: {
  value: BindValue;
  sources: ContentSource[];
  onChange: (next: BindValue) => void;
}) {
  const set = (patch: Partial<BindValue>) => {
    const next: BindValue = { ...value, ...patch };
    if (next.term === "") delete next.term;
    if (next.sort === "") delete next.sort;
    onChange(next);
  };
  const known = sources.some((s) => s.slug === value.source);

  return (
    <div
      className="grid gap-1.5 rounded-md border border-dashed p-2"
      data-testid="binding-editor"
    >
      <p className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
        Data source
      </p>

      <label className="grid grid-cols-[6rem_1fr] items-center gap-2 text-xs">
        <span className="text-muted-foreground">Source</span>
        <select
          aria-label="Binding source"
          value={known ? value.source : ""}
          onChange={(e) => set({ source: e.target.value })}
          className={inputClass}
        >
          <option value="" disabled>
            Choose…
          </option>
          {sources.map((s) => (
            <option key={s.slug} value={s.slug}>
              {s.plural} ({s.count}){s.plugin ? " · plugin" : ""}
            </option>
          ))}
        </select>
      </label>
      {value.source !== undefined && value.source !== "" && !known ? (
        <p className="text-[10px] text-amber-600 dark:text-amber-400">
          &ldquo;{value.source}&rdquo; is not served right now — its plugin
          may be off. The section renders as nothing until it returns.
        </p>
      ) : null}

      <label className="grid grid-cols-[6rem_1fr] items-center gap-2 text-xs">
        <span className="text-muted-foreground">Sort</span>
        <select
          aria-label="Binding sort"
          value={value.sort ?? "newest"}
          onChange={(e) => set({ sort: e.target.value })}
          className={inputClass}
        >
          {SORTS.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
      </label>

      <label className="grid grid-cols-[6rem_1fr] items-center gap-2 text-xs">
        <span className="text-muted-foreground">Limit</span>
        <input
          aria-label="Binding limit"
          type="number"
          min={1}
          max={24}
          value={value.limit ?? 6}
          onChange={(e) => {
            const n = Number(e.target.value);
            set({ limit: Number.isFinite(n) ? Math.min(24, Math.max(1, n)) : undefined });
          }}
          className={inputClass}
        />
      </label>

      <label className="grid grid-cols-[6rem_1fr] items-center gap-2 text-xs">
        <span className="text-muted-foreground">Term</span>
        <input
          aria-label="Binding term"
          type="text"
          placeholder="slug, or taxonomy:slug"
          value={value.term ?? ""}
          onChange={(e) => set({ term: e.target.value })}
          className={inputClass}
        />
      </label>
    </div>
  );
}
