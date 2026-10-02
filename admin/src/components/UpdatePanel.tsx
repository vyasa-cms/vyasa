import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/client";
import { useConfirm } from "@/components/ui/dialog";
import { Chip, Panel, Skeleton } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";

/**
 * Updates: what is running, what is available, and whether this install
 * can upgrade itself.
 *
 * A container cannot replace its own image, so in Docker and Kubernetes
 * this panel shows the commands instead of a button that would lie. Where
 * the button *is* offered, the server restarts underneath it — so the
 * polling below treats a failed request as "still restarting" rather than
 * as an error, and reloads once the new build answers.
 */
export function UpdatePanel() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [upgrading, setUpgrading] = React.useState(false);

  const status = useQuery({
    queryKey: ["updates"],
    queryFn: () => api.updateStatus(),
  });

  // While an upgrade runs, the server it runs on goes away and comes
  // back. A rejected poll is expected, not a failure.
  const progress = useQuery({
    queryKey: ["update-progress"],
    queryFn: () => api.updateProgress().catch(() => null),
    refetchInterval: upgrading ? 2000 : false,
    retry: false,
  });

  React.useEffect(() => {
    const stage = progress.data?.stage;
    if (!upgrading || stage === undefined) return;
    if (stage === "done") {
      setUpgrading(false);
      notify.success("Upgraded", `Now running ${progress.data?.target ?? ""}.`);
      // The bundle is stamped against the binary; reloading picks up both.
      window.location.reload();
    } else if (stage === "failed" || stage === "rolled_back") {
      setUpgrading(false);
      notify.error("Upgrade stopped", progress.data?.detail ?? "");
      void queryClient.invalidateQueries({ queryKey: ["updates"] });
    }
  }, [progress.data, upgrading, queryClient]);

  const start = useMutation({
    mutationFn: (version: string) => api.applyUpdate({ version }),
    onSuccess: () => setUpgrading(true),
    onError: (e) => notify.error("Couldn't start the upgrade", e),
  });

  if (status.isLoading) {
    return (
      <Panel title="Updates">
        <Skeleton className="h-20 w-full" />
      </Panel>
    );
  }
  if (status.data === undefined) {
    return (
      <Panel title="Updates">
        <p className="p-4 text-sm text-muted-foreground">
          Update status unavailable.
        </p>
      </Panel>
    );
  }

  const { current, latest, update_available, releases, environment, instructions, preflight } =
    status.data;
  const next = releases[0];
  const selfManaged = environment.binary_replaceable;

  return (
    <Panel title="Updates">
      <div className="space-y-4 p-4 text-sm">
        <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
          <span>
            Running <span className="font-medium">{current}</span>
          </span>
          {latest !== null ? (
            <span className="text-muted-foreground">
              latest {latest}
              {update_available ? "" : " — up to date"}
            </span>
          ) : (
            <span className="text-muted-foreground">
              {status.data.channel_error === null || status.data.channel_error === undefined || /no update channel/i.test(status.data.channel_error) ? (
                <>
                  no update channel set;{" "}
                  <a href="/admin/settings#settings-updates" className="text-primary underline-offset-2 hover:underline">choose one in Settings</a>
                </>
              ) : (
                `channel unreachable: ${status.data.channel_error}`
              )}
            </span>
          )}
          <Chip tone="neutral" dot={false}>
            {environment.mode}
          </Chip>
        </div>

        {upgrading ? (
          <div className="rounded border border-primary/40 bg-primary/5 p-3" role="status">
            <p className="font-medium">
              Upgrading to {progress.data?.target ?? next?.version}…
            </p>
            <p className="text-muted-foreground">
              {progress.data?.detail ?? "starting"}
            </p>
            <p className="mt-1 text-xs text-muted-foreground">
              The server restarts during this. This page will reload itself.
            </p>
          </div>
        ) : null}

        {update_available && next !== undefined ? (
          <div className="space-y-3">
            <div>
              <p className="font-medium">
                {next.version} is available
                {next.requires_attention ? " — needs attention" : ""}
              </p>
              {next.summary !== "" ? (
                <p className="text-muted-foreground">{next.summary}</p>
              ) : null}
              {next.notes_url !== null ? (
                <a
                  href={next.notes_url}
                  target="_blank"
                  rel="noreferrer"
                  className="text-primary underline"
                >
                  Release notes
                </a>
              ) : null}
            </div>

            {selfManaged ? (
              <button
                type="button"
                disabled={!preflight.can_proceed || upgrading || start.isPending}
                className="rounded bg-primary px-3 py-1.5 text-sm text-primary-foreground disabled:opacity-50"
                onClick={() => {
                  void confirm({
                    title: `Upgrade to ${next.version}?`,
                    description:
                      "The database is backed up first, then the binary is replaced and the server restarts. Migrations cannot be undone — that backup is the way back.",
                  }).then((ok) => {
                    if (ok) start.mutate(next.version);
                  });
                }}
              >
                {start.isPending ? "Starting…" : `Upgrade to ${next.version}`}
              </button>
            ) : (
              <div>
                <p className="text-muted-foreground">
                  This install is upgraded from outside — the image is the unit of
                  upgrade, so nothing here can swap it safely. Run:
                </p>
                <pre className="mt-1 overflow-x-auto rounded bg-muted p-2 text-xs">
                  {instructions.join("\n")}
                </pre>
              </div>
            )}
          </div>
        ) : null}

        <div>
          <p className="text-xs font-medium text-muted-foreground">Preflight</p>
          <ul className="mt-1 space-y-1" data-testid="update-preflight">
            {preflight.findings.map((f) => (
              <li key={f.name} className="flex gap-2">
                <Chip
                  tone={f.status === "ok" ? "success" : f.status === "warn" ? "warning" : "danger"}
                  dot={false}
                >
                  {f.status}
                </Chip>
                <span className="min-w-0 flex-1 text-muted-foreground">{f.detail}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </Panel>
  );
}
