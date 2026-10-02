import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, CheckCircle2, RefreshCw, XCircle } from "lucide-react";
import { api, type HealthCheck } from "@/api/client";
import { UpdatePanel } from "@/components/UpdatePanel";
import { Button } from "@/components/ui/button";
import { Chip, ErrorNote, PageHeader, Panel, SkeletonRows } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/health/")({
  component: HealthPage,
});

/**
 * Human names for the server's stable check identifiers. Keeping the mapping
 * here rather than in the API means a new check still renders — under its
 * raw name — instead of disappearing from the page.
 */
const LABELS: Record<string, string> = {
  database: "Database",
  database_size: "Database size",
  disk_free: "Disk space",
  secret_key: "Secret key",
  storage: "Media directory",
  object_storage: "Object storage",
  media_storage: "Media usage",
  index_dir: "Search directory",
  registry_dir: "Registry directory",
  https: "HTTPS",
  smtp: "Mail relay",
  search_index: "Search index",
  broken_links: "Outbound links",
  link_sweep: "Link sweep",
  stale_locks: "Editing locks",
  expired_sessions: "Sessions",
  job_queue: "Background jobs",
  plugins: "Plugins",
  ai_budget: "AI budget",
  site_url: "Site address",
  scheduled_publisher: "Scheduled publishing",
};

function label(name: string): string {
  return LABELS[name] ?? name.replace(/_/g, " ");
}

const GROUPS: { id: string; title: string }[] = [
  { id: "core", title: "Core" },
  { id: "content", title: "Content" },
  { id: "delivery", title: "Delivery" },
  { id: "assistants", title: "Assistants" },
  { id: "operations", title: "Operations" },
];

const TONE = { ok: "success", warn: "warning", fail: "danger" } as const;
const ICON = { ok: CheckCircle2, warn: AlertTriangle, fail: XCircle } as const;
const SUMMARY = {
  ok: "Everything is working.",
  warn: "Running, but some things need attention.",
  fail: "Something is broken and needs fixing now.",
} as const;
const RANK = { fail: 0, warn: 1, ok: 2 } as const;

function CheckRow({ check, onOp, busy }: { check: HealthCheck; onOp: (op: string) => void; busy: boolean }) {
  const Icon = ICON[check.status];
  const action = check.action;
  return (
    <li className="flex items-start gap-3 py-3" data-testid={`check-${check.name}`}>
      <Icon
        aria-hidden="true"
        className={cn("mt-0.5 h-4 w-4 shrink-0", check.status === "ok" ? "text-success" : check.status === "warn" ? "text-warning" : "text-destructive")}
      />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-medium">{label(check.name)}</span>
          <Chip tone={TONE[check.status]} dot={false}>
            {check.status === "ok" ? "OK" : check.status === "warn" ? "Attention" : "Failing"}
          </Chip>
        </div>
        <p className="mt-1 wrap-break-word text-sm text-muted-foreground">{check.detail}</p>
      </div>
      {action ? (
        action.href ? (
          <a href={action.href} className="shrink-0 rounded-md border px-2.5 py-1.5 text-xs font-medium hover:bg-accent">{action.label}</a>
        ) : action.op ? (
          <Button size="sm" variant="outline" className="shrink-0" disabled={busy} onClick={() => onOp(action.op ?? "")}>
            {busy ? "Working…" : action.label}
          </Button>
        ) : null
      ) : null}
    </li>
  );
}

function when(iso: string | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export function HealthPage() {
  const queryClient = useQueryClient();
  const health = useQuery({
    queryKey: ["site-health"],
    queryFn: () => api.siteHealth(),
    // Operational state goes stale quickly; a page left open should not
    // keep showing an outage that has since cleared.
    refetchInterval: 30_000,
  });
  const op = useMutation({
    mutationFn: (name: string) => api.siteHealthOp(name),
    onSuccess: (result, name) => {
      const summary = Object.entries(result).map(([k, v]) => `${v} ${k}`).join(", ");
      notify.success(name === "reindex" ? "Index rebuilt" : "Cleaned up", summary);
      void queryClient.invalidateQueries({ queryKey: ["site-health"] });
    },
    onError: (e) => notify.error("That didn't work", e),
  });

  const report = health.data;
  const checks = [...(report?.checks ?? [])].sort((a, b) => RANK[a.status] - RANK[b.status]);
  const issues = checks.filter((c) => c.status !== "ok");
  const failures = report?.failures ?? checks.filter((c) => c.status === "fail").length;
  const warnings = report?.warnings ?? checks.filter((c) => c.status === "warn").length;
  const grouped = GROUPS.map((g) => ({ ...g, checks: checks.filter((c) => (c.group ?? "operations") === g.id) })).filter((g) => g.checks.length > 0);
  const ungrouped = checks.filter((c) => c.group !== undefined && !GROUPS.some((g) => g.id === c.group));

  return (
    <div className="space-y-4">
      <PageHeader
        title="Site health"
        description="What this installation can and cannot currently do."
        actions={
          <Button variant="secondary" onClick={() => void health.refetch()} disabled={health.isFetching}>
            <RefreshCw className={health.isFetching ? "h-4 w-4 animate-spin" : "h-4 w-4"} aria-hidden="true" />
            Re-check
          </Button>
        }
      />

      {health.isError ? <ErrorNote title="Couldn't load the health report" error={health.error} /> : null}

      {/* The verdict, first and unmissable. */}
      {report ? (
        <div
          role="status"
          data-testid="health-verdict"
          className={cn(
            "flex flex-wrap items-center gap-x-4 gap-y-1 rounded-lg border px-4 py-3",
            report.status === "ok" ? "border-success/40 bg-success/10" : report.status === "warn" ? "border-warning/40 bg-warning/10" : "border-destructive/40 bg-destructive/10",
          )}
        >
          <span className="font-medium">{SUMMARY[report.status]}</span>
          <span className="text-sm text-muted-foreground">
            {checks.length} checks · {failures} failing · {warnings} need attention
            {when(report.checked_at) ? ` · ran at ${when(report.checked_at)}` : ""}
          </span>
        </div>
      ) : null}

      {issues.length > 0 ? (
        <Panel title="Needs attention" description="Worst first. Each has what to do next." testId="health-issues">
          <ul className="divide-y">
            {issues.map((c) => (
              <CheckRow key={c.name} check={c} busy={op.isPending && op.variables === c.action?.op} onOp={(o) => op.mutate(o)} />
            ))}
          </ul>
        </Panel>
      ) : null}

      <Panel
        title={report === undefined ? "Checking…" : "All checks"}
        description="Each check runs read-only and gives up after five seconds."
        testId="site-health"
      >
        {health.isLoading ? (
          <SkeletonRows rows={6} />
        ) : report === undefined ? null : (
          <div className="grid gap-x-10 gap-y-2 2xl:grid-cols-2">
            {grouped.map((g) => (
              <section key={g.id} aria-labelledby={`health-${g.id}`}>
                <h3 id={`health-${g.id}`} className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">{g.title}</h3>
                <ul className="divide-y">
                  {g.checks.map((c) => (
                    <CheckRow key={c.name} check={c} busy={op.isPending && op.variables === c.action?.op} onOp={(o) => op.mutate(o)} />
                  ))}
                </ul>
              </section>
            ))}
            {ungrouped.length > 0 || (grouped.length === 0 && checks.length > 0) ? (
              <section>
                <ul className="divide-y">
                  {(grouped.length === 0 ? checks : ungrouped).map((c) => (
                    <CheckRow key={c.name} check={c} busy={false} onOp={(o) => op.mutate(o)} />
                  ))}
                </ul>
              </section>
            ) : null}
          </div>
        )}
      </Panel>

      <UpdatePanel />
    </div>
  );
}
