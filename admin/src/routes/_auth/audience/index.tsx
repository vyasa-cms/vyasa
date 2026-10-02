import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Trash2 } from "lucide-react";
import { api } from "@/api/client";
import { useConfirm } from "@/components/ui/dialog";
import { useCapabilities } from "@/lib/capabilities";
import { Chip, EmptyState, ErrorNote, PageHeader, Panel, Skeleton } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";

export const Route = createFileRoute("/_auth/audience/")({
  component: AudiencePage,
});

/**
 * Who reads the site and who raised a hand: cookieless view counts, the
 * lead-form inbox, and the newsletter list. All server-counted — there
 * is no tracking script to consent to.
 */
function AudiencePage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const caps = useCapabilities();
  // The reads (`analytics/summary`, `audience/submissions`, `audience/subscribers`)
  // accept `edit_others`, which is what makes the page visible; deleting either
  // a submission or a subscriber needs `manage_options`.
  const canDelete = caps.can("manage_options");

  const analytics = useQuery({
    queryKey: ["analytics", 30],
    queryFn: () => api.analyticsSummary(30),
  });
  const submissions = useQuery({
    queryKey: ["submissions"],
    queryFn: () => api.listSubmissions(),
  });
  const subscribers = useQuery({
    queryKey: ["subscribers"],
    queryFn: () => api.listSubscribers(),
  });

  const dropSubmission = useMutation({
    mutationFn: (id: string) => api.deleteSubmission(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["submissions"] });
    },
    onError: (e) => notify.error("Couldn't delete the submission", e),
  });
  const dropSubscriber = useMutation({
    mutationFn: (id: string) => api.deleteSubscriber(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["subscribers"] });
    },
    onError: (e) => notify.error("Couldn't remove the subscriber", e),
  });

  const confirmed = (subscribers.data ?? []).filter((s) => s.status === "confirmed").length;
  const maxViews = Math.max(1, ...(analytics.data?.series ?? []).map((d) => d.views));

  return (
    <div className="space-y-6">
      <PageHeader
        title="Audience"
        description="Server-counted views (no cookies, no scripts), form submissions, and newsletter subscribers."
      />

      <Panel title="Views — last 30 days">
        {analytics.isLoading ? (
          <Skeleton className="h-24 w-full" />
        ) : analytics.data === undefined ? (
          <p className="p-4 text-sm text-muted-foreground">Analytics unavailable.</p>
        ) : (
          <div className="space-y-4 p-4">
            <p className="text-sm">
              <span className="text-2xl font-semibold">{analytics.data.total}</span>{" "}
              <span className="text-muted-foreground">
                views in 30 days · {analytics.data.today} today
              </span>
            </p>
            {analytics.data.series.length > 0 ? (
              <div
                className="flex h-24 items-end gap-px"
                role="img"
                aria-label="Daily views bar chart"
                data-testid="views-chart"
              >
                {analytics.data.series.map((d) => (
                  <div
                    key={d.day}
                    title={`${d.day}: ${d.views}`}
                    className="min-w-1 flex-1 rounded-t bg-primary/70"
                    style={{ height: `${Math.max(4, (d.views / maxViews) * 100)}%` }}
                  />
                ))}
              </div>
            ) : (
              <p className="text-sm text-muted-foreground">
                No views recorded yet — counts begin with the next visitor.
              </p>
            )}
            <div className="grid gap-4 sm:grid-cols-2">
              <TopList label="Top content" rows={analytics.data.top_paths} />
              <TopList label="Top referrers" rows={analytics.data.top_referrers} />
            </div>
          </div>
        )}
      </Panel>

      <Panel title="Form inbox">
        {submissions.isError ? <ErrorNote title="Couldn't load submissions" error={submissions.error} onRetry={() => void submissions.refetch()} /> : submissions.isLoading ? (
          <Skeleton className="h-16 w-full" />
        ) : (submissions.data ?? []).length === 0 ? (
          <EmptyState
            title="No submissions yet"
            description="Add a “Signup form” section to any page in the studio; what visitors send lands here."
          />
        ) : (
          <ul className="divide-y">
            {(submissions.data ?? []).map((s) => (
              <li key={s.id} className="flex items-start gap-3 p-3 text-sm">
                <div className="min-w-0 flex-1">
                  <p className="font-medium">
                    {s.name === "" ? s.email : `${s.name} · ${s.email}`}
                    <Chip tone="neutral" dot={false} className="ml-2">
                      {s.form}
                    </Chip>
                  </p>
                  {s.message !== "" ? (
                    <p className="mt-1 whitespace-pre-wrap text-muted-foreground">{s.message}</p>
                  ) : null}
                  <p className="mt-1 text-xs text-muted-foreground">
                    {new Date(s.created_at).toLocaleString()}
                  </p>
                </div>
                {canDelete ? (
                  <button
                    type="button"
                    aria-label={`Delete submission from ${s.email}`}
                    className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-destructive"
                    onClick={() => {
                      void confirm({
                        title: "Delete this submission?",
                        description: "There is no undo.",
                        destructive: true,
                      }).then((ok) => {
                        if (ok) dropSubmission.mutate(s.id);
                      });
                    }}
                  >
                    <Trash2 className="h-4 w-4" aria-hidden="true" />
                  </button>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </Panel>

      <Panel title={`Subscribers${confirmed > 0 ? ` — ${confirmed} confirmed` : ""}`}>
        {subscribers.isError ? <ErrorNote title="Couldn't load subscribers" error={subscribers.error} onRetry={() => void subscribers.refetch()} /> : subscribers.isLoading ? (
          <Skeleton className="h-16 w-full" />
        ) : (subscribers.data ?? []).length === 0 ? (
          <EmptyState
            title="Nobody has subscribed yet"
            description="Add a “Signup form” section in newsletter mode, and switch the newsletter on in Settings."
          />
        ) : (
          <ul className="divide-y">
            {(subscribers.data ?? []).map((s) => (
              <li key={s.id} className="flex items-center gap-3 p-3 text-sm">
                <span className="min-w-0 flex-1 truncate">{s.email}</span>
                <Chip
                  tone={
                    s.status === "confirmed"
                      ? "success"
                      : s.status === "pending"
                        ? "neutral"
                        : "danger"
                  }
                  dot={false}
                >
                  {s.status}
                </Chip>
                {canDelete ? (
                  <button
                    type="button"
                    aria-label={`Remove ${s.email}`}
                    className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-destructive"
                    onClick={() => {
                      void confirm({
                        title: `Remove ${s.email}?`,
                        description: "They can sign up again any time.",
                        destructive: true,
                      }).then((ok) => {
                        if (ok) dropSubscriber.mutate(s.id);
                      });
                    }}
                  >
                    <Trash2 className="h-4 w-4" aria-hidden="true" />
                  </button>
                ) : null}
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </div>
  );
}

function TopList({ label, rows }: { label: string; rows: { name: string; views: number }[] }) {
  return (
    <div>
      <p className="text-xs font-medium text-muted-foreground">{label}</p>
      {rows.length === 0 ? (
        <p className="mt-1 text-sm text-muted-foreground">Nothing yet.</p>
      ) : (
        <ul className="mt-1 space-y-1 text-sm">
          {rows.slice(0, 6).map((r) => (
            <li key={r.name} className="flex items-baseline justify-between gap-2">
              <span className="min-w-0 truncate">{r.name}</span>
              <span className="text-muted-foreground">{r.views}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
