import { Database, FileText, Package } from "lucide-react";

import type { Vocabulary } from "@/api/themes";

/**
 * The Data panel: every content source a binding may draw from.
 *
 * Read straight from the vocabulary — the same list the binding editor
 * offers and the assistants are told about — so all three always agree.
 */
export function DataPanel({ vocab }: { vocab: Vocabulary }) {
  const sources = vocab.sources ?? [];
  return (
    <div className="space-y-2" data-testid="data-panel">
      {sources.length === 0 ? (
        <p className="text-[11px] text-muted-foreground">
          This server predates content sources; update it to bind sections
          to data.
        </p>
      ) : (
        <ul className="space-y-1">
          {sources.map((s) => {
            const Icon = s.plugin ? Package : FileText;
            return (
              <li
                key={s.slug}
                className="flex items-center gap-2.5 rounded-md border bg-card px-2.5 py-2"
              >
                <span className="grid h-7 w-7 shrink-0 place-items-center rounded-md bg-muted text-muted-foreground">
                  <Icon className="h-3.5 w-3.5" aria-hidden="true" />
                </span>
                <span className="min-w-0">
                  <span className="block truncate text-xs font-medium">{s.plural}</span>
                  <span className="block font-mono text-[10px] text-muted-foreground">
                    {s.slug} · {s.count} published
                  </span>
                </span>
                {s.plugin ? (
                  <span className="ml-auto rounded bg-primary/10 px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wider text-primary">
                    Plugin
                  </span>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
      <p className="flex items-start gap-2 rounded-md border border-dashed px-2 py-1.5 text-[10px] leading-relaxed text-muted-foreground">
        <Database className="mt-0.5 h-3 w-3 shrink-0" aria-hidden="true" />
        <span>
          Any <b>collection</b> section binds to one of these — source,
          sort, limit — and stays live as entries change. Plugins bring
          their own sources when enabled.
        </span>
      </p>
    </div>
  );
}
