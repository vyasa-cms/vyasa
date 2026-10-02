import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery } from "@tanstack/react-query";
import { api, type AuditEntry, type PersonalData } from "@/api/client";
import { Button } from "@/components/ui/button";
import { useConfirm } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Chip, Field, PageHeader, Panel, ErrorNote } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";

export const Route = createFileRoute("/_auth/privacy/")({ component: PrivacyPage });

/**
 * Two things a site owner is asked for: what do you hold about me, and
 * who did what. Personal data by email address, and the audit log.
 */
export function PrivacyPage() {
  return (
    <div className="space-y-5">
      <PageHeader title="Privacy & audit" description="What the site holds about a person, how to hand it over or erase it, and who changed what." />
      <div className="grid gap-5 2xl:grid-cols-2">
        <PersonalDataPanel />
        <PolicyPanel />
      </div>
      <AuditPanel />
    </div>
  );
}

function PersonalDataPanel() {
  const confirm = useConfirm();
  const [email, setEmail] = React.useState("");
  const [data, setData] = React.useState<PersonalData | null>(null);
  const look = useMutation({
    mutationFn: () => api.privacyExport(email.trim()),
    onSuccess: setData,
    onError: (e) => notify.error("Couldn't look that up", e),
  });
  const erase = useMutation({
    mutationFn: () => api.privacyErase(email.trim()),
    onSuccess: (r) => {
      setData(null);
      notify.success("Erased", `${r.comments_anonymised} comments anonymised, ${r.submissions_deleted} messages and ${r.subscriptions_deleted} subscriptions deleted${r.account_deleted ? ", account deleted" : ""}.${r.note ? ` ${r.note}` : ""}`);
    },
    onError: (e) => notify.error("Couldn't erase", e),
  });
  const counts = data ? [
    ["account", data.account ? 1 : 0],
    ["posts", data.posts.length],
    ["comments", data.comments.length],
    ["messages", data.submissions.length],
    ["subscriptions", data.subscriptions.length],
  ] as const : [];
  return (
    <Panel title="Personal data" description="Look up an email address to see everything the site holds about it. Export hands it over as a file; erase anonymises comments and deletes the rest." testId="personal-data">
      <div className="space-y-3">
        <form className="flex flex-wrap items-end gap-2" onSubmit={(e) => { e.preventDefault(); look.mutate(); }}>
          <div className="min-w-[16rem] flex-1"><Field label="Email address" htmlFor="pd-email"><Input id="pd-email" type="email" value={email} onChange={(e) => { setEmail(e.target.value); setData(null); }} placeholder="person@example.com" /></Field></div>
          <Button type="submit" size="sm" disabled={email.trim() === "" || look.isPending}>{look.isPending ? "Looking…" : "Look up"}</Button>
        </form>
        {data ? (
          <div className="space-y-2 text-sm" data-testid="personal-data-result">
            <p className="flex flex-wrap gap-1">{counts.map(([k, n]) => <Chip key={k} tone={n > 0 ? "info" : "neutral"} dot={false}>{n} {k}</Chip>)}</p>
            {counts.every(([, n]) => n === 0) ? <p className="text-muted-foreground">Nothing is held for this address.</p> : null}
            <div className="flex flex-wrap gap-2">
              <a href={`/api/v1/privacy/export?email=${encodeURIComponent(email.trim())}`} className="inline-flex h-8 items-center rounded-md border px-3 text-sm hover:bg-accent" download>Download as JSON</a>
              <Button size="sm" variant="outline" className="text-destructive" disabled={erase.isPending} onClick={async () => { if (await confirm({ title: `Erase everything for ${email.trim()}?`, description: "Comments stay but lose their name and address. Messages, subscriptions and the account itself are deleted. This cannot be undone.", confirmLabel: "Erase", destructive: true, requireTyped: email.trim() })) erase.mutate(); }}>Erase</Button>
            </div>
          </div>
        ) : null}
      </div>
    </Panel>
  );
}

function PolicyPanel() {
  const make = useMutation({
    mutationFn: () => api.privacyPolicyPage(),
    onSuccess: (r) => notify.success(r.created ? "Draft written" : "Already there", r.created ? "A Privacy policy page is waiting in Pages as a draft; read it, adjust it, publish it." : "A page with the slug privacy-policy already exists."),
    onError: (e) => notify.error("Couldn't create the page", e),
  });
  return (
    <Panel title="Privacy policy" description="A starter page that says what this software actually does: no cookies on the public site, what comments and forms keep, how to ask for erasure. It is written as a draft for you to finish.">
      <div className="space-y-2 text-sm">
        <p className="text-muted-foreground">Readers of the public site get no cookies and no third-party scripts, so no consent banner is needed. The admin uses a session cookie for signed-in staff only.</p>
        <Button size="sm" variant="outline" disabled={make.isPending} onClick={() => make.mutate()}>{make.isPending ? "Writing…" : "Write a draft policy page"}</Button>
      </div>
    </Panel>
  );
}

function AuditPanel() {
  const [q, setQ] = React.useState("");
  const log = useQuery({ queryKey: ["audit-log"], queryFn: () => api.auditLog(300), refetchInterval: 60_000 });
  const rows = (log.data ?? []).filter((r: AuditEntry) => q === "" || `${r.actor_name} ${r.action} ${r.target}`.toLowerCase().includes(q.toLowerCase()));
  return (
    <Panel title="Audit log" description="Who did what: role changes, deletions, plugin and theme changes, option writes, exports and erasures. Newest first." testId="audit-log">
      <div className="space-y-2">
        <Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Filter by person, action or target" aria-label="Filter the audit log" className="max-w-md" />
        {log.isError ? <ErrorNote title="Couldn't load audit log" error={log.error} onRetry={() => void log.refetch()} /> : rows.length === 0 ? <p className="text-sm text-muted-foreground">{log.isPending ? "Loading…" : "Nothing recorded yet."}</p> : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead><tr className="text-left text-[11px] uppercase tracking-wide text-muted-foreground"><th className="pb-1 font-medium">When</th><th className="pb-1 font-medium">Who</th><th className="pb-1 font-medium">Action</th><th className="pb-1 font-medium">Target</th><th className="pb-1 font-medium">Detail</th></tr></thead>
              <tbody className="divide-y">
                {rows.map((r) => (
                  <tr key={r.id}>
                    <td className="whitespace-nowrap py-1 pr-3 text-xs text-muted-foreground">{new Date(r.created_at).toLocaleString()}</td>
                    <td className="py-1 pr-3">{r.actor_name || "system"}</td>
                    <td className="py-1 pr-3 font-mono text-xs">{r.action}</td>
                    <td className="py-1 pr-3 font-mono text-xs text-muted-foreground">{r.target}</td>
                    <td className="max-w-[28rem] truncate py-1 text-xs text-muted-foreground" title={JSON.stringify(r.detail)}>{Object.keys(r.detail).length > 0 ? JSON.stringify(r.detail) : ""}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </Panel>
  );
}
