import * as React from "react";
import { errorSummary } from "@/lib/error-text";
import { cn } from "@/lib/utils";

/* ------------------------------------------------------------------ chips */

type Tone = "neutral" | "success" | "warning" | "danger" | "info";

const TONE_CLASS: Record<Tone, string> = {
  neutral: "bg-muted text-muted-foreground",
  success: "bg-success-subtle text-success",
  warning: "bg-warning-subtle text-warning",
  danger: "bg-destructive-subtle text-destructive",
  info: "bg-primary-subtle text-primary",
};

/**
 * State as form, not just colour: a dot plus a label, so status survives
 * greyscale and low-vision viewing.
 */
export function Chip({
  children,
  tone = "neutral",
  dot = true,
  className,
}: {
  children: React.ReactNode;
  tone?: Tone;
  dot?: boolean;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex max-w-full items-center gap-1.5 truncate rounded-full px-2 py-0.5 text-xs font-medium",
        TONE_CLASS[tone],
        className,
      )}
    >
      {dot ? (
        <span
          aria-hidden="true"
          className="h-1.5 w-1.5 shrink-0 rounded-full bg-current"
        />
      ) : null}
      <span className="truncate">{children}</span>
    </span>
  );
}

const POST_STATUS_TONE: Record<string, Tone> = {
  published: "success",
  scheduled: "warning",
  private: "info",
  trash: "danger",
  draft: "neutral",
  pending: "warning",
  approved: "success",
  spam: "danger",
};

/** Maps a post/comment status onto a chip with sentence-case wording. */
export function StatusChip({ status }: { status: string }) {
  const tone = POST_STATUS_TONE[status] ?? "neutral";
  const label = status.charAt(0).toUpperCase() + status.slice(1);
  return <Chip tone={tone}>{label}</Chip>;
}

/* -------------------------------------------------------------- skeletons */

export function Skeleton({ className }: { className?: string }) {
  return (
    <div
      aria-hidden="true"
      className={cn("animate-pulse rounded-md bg-muted", className)}
    />
  );
}

/** Placeholder rows sized like the table they stand in for. */
export function SkeletonRows({ rows = 5 }: { rows?: number }) {
  return (
    <div className="divide-y" data-testid="skeleton-rows">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center gap-3 px-3 py-3 sm:px-4">
          <Skeleton className="h-4 w-4 shrink-0 rounded" />
          <div className="min-w-0 flex-1 space-y-2">
            <Skeleton className="h-3.5 w-1/2" />
            <Skeleton className="h-3 w-1/3" />
          </div>
          <Skeleton className="hidden h-5 w-20 rounded-full sm:block" />
        </div>
      ))}
    </div>
  );
}

/* ------------------------------------------------------------ empty state */

/**
 * An empty list is an invitation, not a dead end — it names the thing and
 * offers the action that creates one.
 */
export function EmptyState({
  icon: Icon,
  title,
  description,
  action,
  testId,
}: {
  icon?: React.ComponentType<{ className?: string }>;
  title: string;
  description?: string;
  action?: React.ReactNode;
  testId?: string;
}) {
  return (
    <div
      data-testid={testId}
      className="flex flex-col items-center justify-center gap-3 px-6 py-12 text-center"
    >
      {Icon !== undefined ? (
        <span className="flex h-11 w-11 items-center justify-center rounded-full bg-muted text-muted-foreground">
          <Icon className="h-5 w-5" />
        </span>
      ) : null}
      <div className="space-y-1">
        <p className="text-sm font-medium">{title}</p>
        {description !== undefined ? (
          <p className="mx-auto max-w-sm text-sm text-muted-foreground">
            {description}
          </p>
        ) : null}
      </div>
      {action}
    </div>
  );
}

/* ------------------------------------------------------------ page header */

/**
 * One header for every screen: title, a supporting line of real numbers, and
 * the page's actions. Actions wrap below the title on phones.
 */
export function PageHeader({
  title,
  description,
  actions,
  children,
}: {
  title: string;
  description?: React.ReactNode;
  actions?: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:gap-4">
      <div className="min-w-0 flex-1">
        <h1 className="truncate text-xl font-semibold tracking-tight sm:text-2xl">
          {title}
        </h1>
        {description !== undefined ? (
          <div className="mt-1 text-sm text-muted-foreground">{description}</div>
        ) : null}
        {children}
      </div>
      {actions !== undefined ? (
        <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>
      ) : null}
    </div>
  );
}

/* ----------------------------------------------------------------- fields */

/**
 * A labelled field. Placeholders are hints, never labels — they disappear the
 * moment someone types.
 */
export function Field({
  label,
  hint,
  error,
  htmlFor,
  children,
  className,
}: {
  label: string;
  hint?: string;
  error?: string | null;
  htmlFor?: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("space-y-1.5", className)}>
      <label
        htmlFor={htmlFor}
        className="block text-sm font-medium leading-none"
      >
        {label}
      </label>
      {children}
      {error !== undefined && error !== null ? (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      ) : hint !== undefined ? (
        <p className="text-xs text-muted-foreground">{hint}</p>
      ) : null}
    </div>
  );
}

/** Bordered container for a group of related settings or a create form. */
export function Panel({
  title,
  description,
  footer,
  children,
  className,
  testId,
}: {
  title?: string;
  description?: string;
  footer?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
  testId?: string;
}) {
  return (
    <section
      data-testid={testId}
      className={cn("rounded-lg border bg-card text-card-foreground", className)}
    >
      {title !== undefined ? (
        <div className="border-b px-4 py-3 sm:px-5">
          <h2 className="text-sm font-semibold">{title}</h2>
          {description !== undefined ? (
            <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>
          ) : null}
        </div>
      ) : null}
      <div className="p-4 sm:p-5">{children}</div>
      {footer !== undefined ? (
        <div className="flex flex-wrap justify-end gap-2 border-t bg-muted/40 px-4 py-3 sm:px-5">
          {footer}
        </div>
      ) : null}
    </section>
  );
}

/** Inline error band used when a query fails inside an otherwise-live page. */
export function ErrorNote({
  title = "Something went wrong",
  error,
  onRetry,
}: {
  title?: string;
  error: unknown;
  onRetry?: () => void;
}) {
  const { summary, rest } = errorSummary(error);
  return (
    <div
      role="alert"
      data-testid="query-error"
      className="rounded-lg border border-destructive/40 bg-destructive-subtle px-4 py-3 text-sm text-destructive"
    >
      <p className="font-medium">{title}</p>
      {onRetry ? <button type="button" className="mt-2 underline" onClick={onRetry}>Try again</button> : null}
      <p className="mt-0.5 break-words opacity-90">{summary}</p>
      {rest === null ? null : (
        <details className="mt-1">
          <summary className="cursor-pointer text-xs opacity-70">Details</summary>
          <p className="mt-1 whitespace-pre-wrap break-words font-mono text-[11px] opacity-80">
            {rest}
          </p>
        </details>
      )}
    </div>
  );
}
