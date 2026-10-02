import * as React from "react";
import { ChevronLeft, ChevronRight, Search, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { SkeletonRows } from "@/components/ui/primitives";
import { useIsMobile } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

/** A column definition. `render` receives the row and returns cell content. */
export interface Column<T> {
  key: string;
  header: React.ReactNode;
  render: (row: T) => React.ReactNode;
  /** Hide this column below the given breakpoint; it moves into the mobile card. */
  hideBelow?: "sm" | "md" | "lg";
  /** Grid track width for this column on desktop, e.g. "8rem" or "minmax(0,1fr)". */
  width?: string;
  align?: "left" | "right";
  /** The column that titles the row on phones. Exactly one should set this. */
  primary?: boolean;
}

const HIDE_CLASS = {
  sm: "hidden sm:flex",
  md: "hidden md:flex",
  lg: "hidden lg:flex",
} as const;

export interface DataListProps<T> {
  rows: T[];
  columns: Column<T>[];
  rowKey: (row: T) => string | number;
  /** Whole-row link/action target. */
  onRowClick?: (row: T) => void;
  /** Per-row trailing controls (edit, view, delete…). */
  rowActions?: (row: T) => React.ReactNode;

  isLoading?: boolean;
  error?: unknown;
  empty?: React.ReactNode;

  /** Controlled search box; omit to hide it. */
  search?: { value: string; onChange: (v: string) => void; placeholder?: string };
  /** Filter controls rendered in the toolbar. */
  filters?: React.ReactNode;

  /** Enables checkboxes; `bulkActions` renders while a selection exists. */
  selection?: {
    selected: Set<string | number>;
    onChange: (next: Set<string | number>) => void;
    bulkActions: (selected: Set<string | number>) => React.ReactNode;
  };

  pagination?: {
    page: number;
    totalPages: number;
    total?: number;
    onPage: (page: number) => void;
    label?: string;
  };

  testId?: string;
  rowTestId?: string | ((row: T) => string);
  totalTestId?: string;
}

/**
 * The admin's one list surface. Table-like grid from `sm` up; stacked cards
 * below it, because a six-column table on a 375px screen is unreadable no
 * matter how much it scrolls.
 */
export function DataList<T>({
  rows,
  columns,
  rowKey,
  onRowClick,
  rowActions,
  isLoading = false,
  error,
  empty,
  search,
  filters,
  selection,
  pagination,
  testId,
  rowTestId,
  totalTestId,
}: DataListProps<T>) {
  const isMobile = useIsMobile();
  const gridTemplate = React.useMemo(() => {
    const tracks = columns.map((c) => c.width ?? "minmax(0,1fr)");
    if (selection !== undefined) tracks.unshift("1.25rem");
    if (rowActions !== undefined) tracks.push("auto");
    return tracks.join(" ");
  }, [columns, selection, rowActions]);

  const allKeys = React.useMemo(() => rows.map(rowKey), [rows, rowKey]);
  const selectedCount = selection?.selected.size ?? 0;
  const allSelected = allKeys.length > 0 && selectedCount === allKeys.length;

  const toggleAll = () => {
    if (selection === undefined) return;
    selection.onChange(allSelected ? new Set() : new Set(allKeys));
  };

  const toggleOne = (key: string | number) => {
    if (selection === undefined) return;
    const next = new Set(selection.selected);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    selection.onChange(next);
  };

  const primary = columns.find((c) => c.primary) ?? columns[0];
  const secondary = columns.filter((c) => c !== primary);

  const hasToolbar = search !== undefined || filters !== undefined;

  return (
    <div
      data-testid={testId}
      className="overflow-hidden rounded-lg border bg-card text-card-foreground"
    >
      {hasToolbar ? (
        <div className="flex flex-col gap-2 border-b bg-muted/40 p-2 sm:flex-row sm:items-center sm:p-2.5">
          {search !== undefined ? (
            <div className="relative min-w-0 flex-1 sm:max-w-xs">
              <Search
                className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
                aria-hidden="true"
              />
              <input
                type="search"
                value={search.value}
                onChange={(e) => search.onChange(e.target.value)}
                placeholder={search.placeholder ?? "Search"}
                aria-label={search.placeholder ?? "Search"}
                className="h-9 w-full rounded-md border border-input bg-background pl-8 pr-8 text-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              />
              {search.value !== "" ? (
                <button
                  type="button"
                  aria-label="Clear search"
                  onClick={() => search.onChange("")}
                  className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-1 text-muted-foreground hover:text-foreground"
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              ) : null}
            </div>
          ) : null}
          {filters !== undefined ? (
            <div className="scrollbar-thin -mx-2 flex items-center gap-2 overflow-x-auto px-2 sm:mx-0 sm:ml-auto sm:px-0">
              {filters}
            </div>
          ) : null}
        </div>
      ) : null}

      {selection !== undefined && selectedCount > 0 ? (
        <div
          data-testid="bulk-bar"
          className="flex flex-wrap items-center gap-2 border-b bg-primary-subtle px-3 py-2 text-sm text-primary"
        >
          <span className="font-medium">{selectedCount} selected</span>
          <div className="flex flex-wrap items-center gap-2">
            {selection.bulkActions(selection.selected)}
          </div>
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto h-7 text-primary hover:bg-primary/10"
            onClick={() => selection.onChange(new Set())}
          >
            Clear
          </Button>
        </div>
      ) : null}

      {/* Column headers — desktop only; cards carry their own labels. */}
      {!isLoading && !isMobile && rows.length > 0 ? (
        <div
          className="grid items-center gap-3 border-b bg-muted/40 px-4 py-2 text-[11px] font-medium uppercase tracking-wider text-muted-foreground"
          style={{ gridTemplateColumns: gridTemplate }}
        >
          {selection !== undefined ? (
            <input
              type="checkbox"
              checked={allSelected}
              onChange={toggleAll}
              aria-label="Select all rows"
              className="h-3.5 w-3.5 cursor-pointer rounded border-input accent-primary"
            />
          ) : null}
          {columns.map((c) => (
            <span
              key={c.key}
              className={cn(
                "truncate",
                c.align === "right" && "text-right",
                c.hideBelow !== undefined && HIDE_CLASS[c.hideBelow],
              )}
            >
              {c.header}
            </span>
          ))}
          {rowActions !== undefined ? <span className="sr-only">Actions</span> : null}
        </div>
      ) : null}

      {isLoading ? (
        <SkeletonRows />
      ) : error !== undefined && error !== null ? (
        <div
          role="alert"
          data-testid="query-error"
          className="px-4 py-6 text-sm text-destructive"
        >
          Couldn&rsquo;t load this list:{" "}
          {error instanceof Error ? error.message : String(error)}
        </div>
      ) : rows.length === 0 ? (
        (empty ?? null)
      ) : (
        <ul className="divide-y">
          {rows.map((row) => {
            const key = rowKey(row);
            const isSelected = selection?.selected.has(key) ?? false;
            return (
              <li
                key={key}
                data-testid={
                  typeof rowTestId === "function" ? rowTestId(row) : rowTestId
                }
                className={cn(
                  "group relative transition-colors",
                  isSelected ? "bg-primary-subtle/60" : "hover:bg-muted/50",
                )}
              >
                {isMobile ? (
                  /* Stacked card: primary column as the title, the rest as meta. */
                  <div className="flex items-start gap-3 p-3">
                    {selection !== undefined ? (
                      <input
                        type="checkbox"
                        checked={isSelected}
                        onChange={() => toggleOne(key)}
                        aria-label="Select row"
                        className="mt-1 h-4 w-4 shrink-0 cursor-pointer rounded border-input accent-primary"
                      />
                    ) : null}
                    <div className="min-w-0 flex-1 space-y-1.5">
                      <div className="text-sm font-medium">{primary?.render(row)}</div>
                      <dl className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
                        {secondary.map((c) => (
                          <div key={c.key} className="flex min-w-0 items-center gap-1">
                            <dt className="sr-only">{c.key}</dt>
                            <dd className="min-w-0 truncate">{c.render(row)}</dd>
                          </div>
                        ))}
                      </dl>
                    </div>
                    {rowActions !== undefined ? (
                      <div className="flex shrink-0 items-center gap-1">
                        {rowActions(row)}
                      </div>
                    ) : null}
                  </div>
                ) : (
                  <div
                    className="grid items-center gap-3 px-4 py-2.5"
                    style={{ gridTemplateColumns: gridTemplate }}
                  >
                    {selection !== undefined ? (
                      <input
                        type="checkbox"
                        checked={isSelected}
                        onChange={() => toggleOne(key)}
                        aria-label="Select row"
                        className="h-3.5 w-3.5 cursor-pointer rounded border-input accent-primary"
                      />
                    ) : null}
                    {columns.map((c) => (
                      <div
                        key={c.key}
                        className={cn(
                          "flex min-w-0 items-center text-sm",
                          c.align === "right" && "justify-end text-right",
                          c.hideBelow !== undefined && HIDE_CLASS[c.hideBelow],
                        )}
                        onClick={
                          onRowClick === undefined
                            ? undefined
                            : () => onRowClick(row)
                        }
                      >
                        <div className="min-w-0 truncate">{c.render(row)}</div>
                      </div>
                    ))}
                    {rowActions !== undefined ? (
                      <div className="flex items-center justify-end gap-1">
                        {rowActions(row)}
                      </div>
                    ) : null}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {pagination !== undefined && !isLoading && rows.length > 0 ? (
        <div className="flex flex-col gap-2 border-t bg-muted/40 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between">
          <span
            data-testid={totalTestId}
            className="text-xs text-muted-foreground"
          >
            {pagination.label ??
              `${pagination.total ?? rows.length} items · page ${pagination.page} of ${pagination.totalPages}`}
          </span>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              className="flex-1 sm:flex-none"
              disabled={pagination.page <= 1}
              onClick={() => pagination.onPage(pagination.page - 1)}
              data-testid="page-prev"
            >
              <ChevronLeft className="h-4 w-4" aria-hidden="true" />
              Previous
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="flex-1 sm:flex-none"
              disabled={pagination.page >= pagination.totalPages}
              onClick={() => pagination.onPage(pagination.page + 1)}
              data-testid="page-next"
            >
              Next
              <ChevronRight className="h-4 w-4" aria-hidden="true" />
            </Button>
          </div>
        </div>
      ) : null}
    </div>
  );
}

/** Small pill-shaped select used for list filters. */
export function FilterSelect({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: { value: string; label: string }[];
  onChange: (value: string) => void;
}) {
  return (
    <label className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
      <span className="sr-only sm:not-sr-only">{label}</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label={label}
        className="h-9 rounded-md border border-input bg-background px-2 text-sm text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring sm:h-8"
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}
