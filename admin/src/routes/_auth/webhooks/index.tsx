import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Activity, Plus, Webhook as WebhookIcon } from "lucide-react";
import { api, type WebhookCreated, type WebhookDelivery, type WebhookSummary } from "@/api/client";
import { Button } from "@/components/ui/button";
import { DataList, type Column } from "@/components/ui/data-list";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, EmptyState, ErrorNote, Field, PageHeader, Skeleton } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/webhooks/")({
  component: WebhooksPage,
});

type Hook = WebhookSummary;

/**
 * The events the server publishes, in the user's language. These names
 * must match `WebhookEvent::ALL` on the server, which rejects any other.
 */
const EVENTS = [
  { value: "post.published", label: "A post is published" },
  { value: "post.updated", label: "A post is updated" },
  { value: "post.trashed", label: "A post is trashed" },
  { value: "comment.added", label: "A comment is left" },
  { value: "theme.changed", label: "A theme is activated" },
  { value: "test.ping", label: "Test" },
];

function eventLabel(value: string): string {
  return EVENTS.find((e) => e.value === value)?.label ?? value;
}

function ago(iso: string): string {
  const ms = Date.now() - new Date(iso).getTime();
  if (!Number.isFinite(ms)) return "";
  const m = Math.round(ms / 60_000);
  if (m < 1) return "just now";
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 48) return `${h} h ago`;
  return `${Math.round(h / 24)} days ago`;
}

const STATUS_LABEL = { success: "Delivered", failed: "Failed", dead: "Gave up" } as const;
const STATUS_TONE = { success: "success", failed: "warning", dead: "danger" } as const;

function WebhooksPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [creating, setCreating] = React.useState(false);
  const [editing, setEditing] = React.useState<Hook | null>(null);
  const [logFor, setLogFor] = React.useState<Hook | null>(null);
  const [secret, setSecret] = React.useState<{ url: string; secret: string; header: string; rotated: boolean } | null>(null);

  const hooks = useQuery({ queryKey: ["webhooks"], queryFn: () => api.listWebhooks(), refetchInterval: 30_000 });
  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["webhooks"] });

  const remove = useMutation({
    mutationFn: (id: string) => api.deleteWebhook(id),
    onSuccess: () => { invalidate(); notify.success("Webhook removed"); },
    onError: (e) => notify.error("Couldn't remove the webhook", e),
  });
  const toggle = useMutation({
    mutationFn: (h: Hook) => api.updateWebhook(h.id, { enabled: !h.enabled }),
    onSuccess: (h) => { invalidate(); notify.success(h.enabled ? "Resumed" : "Paused", h.enabled ? "Deliveries will go out again." : "Nothing is sent until you resume."); },
    onError: (e) => notify.error("Couldn't change the webhook", e),
  });
  const test = useMutation({
    mutationFn: (id: string) => api.testWebhook(id),
    onSuccess: (_r, id) => {
      notify.success("Test queued", "The result lands in the delivery history within a few seconds.");
      setTimeout(() => { invalidate(); void queryClient.invalidateQueries({ queryKey: ["webhook-deliveries", id] }); }, 3000);
    },
    onError: (e) => notify.error("Couldn't queue the test", e),
  });
  const rotate = useMutation({
    mutationFn: (h: Hook) => api.rotateWebhookSecret(h.id).then((r) => ({ ...r, url: h.url })),
    onSuccess: (r) => setSecret({ url: r.url, secret: r.secret, header: r.signature_header, rotated: true }),
    onError: (e) => notify.error("Couldn't rotate the secret", e),
  });

  const askDelete = async (h: Hook) => {
    const ok = await confirm({ title: "Remove this webhook?", description: `${h.url} will stop receiving notifications, and its delivery history is deleted.`, confirmLabel: "Remove", destructive: true });
    if (ok) remove.mutate(h.id);
  };
  const askRotate = async (h: Hook) => {
    const ok = await confirm({ title: "Rotate the signing secret?", description: "The receiver must be updated with the new secret; deliveries signed with the old one will fail verification there.", confirmLabel: "Rotate" });
    if (ok) rotate.mutate(h);
  };

  const rows = hooks.data ?? [];
  const failing = rows.filter((h) => h.enabled && h.last_delivery && h.last_delivery.status !== "success").length;

  const columns: Column<Hook>[] = [
    {
      key: "url",
      header: "Endpoint",
      primary: true,
      render: (h) => (
        <span className="min-w-0">
          <span className="block truncate font-mono text-xs">{h.url}</span>
          <span className="mt-1 flex flex-wrap gap-1">
            {h.events.length === 0 ? <span className="text-[11px] text-muted-foreground">No events selected</span> : h.events.map((e) => (
              <span key={e} className="rounded-full border px-1.5 py-px text-[11px] text-muted-foreground">{eventLabel(e)}</span>
            ))}
          </span>
        </span>
      ),
    },
    {
      key: "enabled",
      header: "State",
      width: "7rem",
      render: (h) => <Chip tone={h.enabled ? "success" : "neutral"}>{h.enabled ? "Active" : "Paused"}</Chip>,
    },
    {
      key: "last",
      header: "Last delivery",
      width: "14rem",
      render: (h) =>
        h.last_delivery ? (
          <span className="flex items-center gap-2 text-xs">
            <Chip tone={STATUS_TONE[h.last_delivery.status]} dot={false}>{STATUS_LABEL[h.last_delivery.status]}</Chip>
            <span className="text-muted-foreground">{ago(h.last_delivery.at)}</span>
          </span>
        ) : (
          <span className="text-xs text-muted-foreground">Never</span>
        ),
    },
  ];

  const action = (label: string, onClick: () => void, opts: { disabled?: boolean; danger?: boolean } = {}) => (
    <button
      type="button"
      onClick={onClick}
      disabled={opts.disabled}
      className={cn("rounded-md px-2 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50", opts.danger ? "text-destructive hover:bg-destructive-subtle" : "text-muted-foreground hover:text-foreground")}
    >
      {label}
    </button>
  );

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Webhooks"
        description="Send a signed notification to another service when something happens on your site."
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <Plus className="h-4 w-4" aria-hidden="true" />
            Add webhook
          </Button>
        }
      />

      {failing > 0 ? (
        <p role="status" className="rounded-md border border-warning/40 bg-warning-subtle px-3 py-2 text-sm text-warning" data-testid="webhooks-failing">
          {failing} {failing === 1 ? "webhook's" : "webhooks'"} last delivery failed. Open its history to see what the receiver said.
        </p>
      ) : null}

      <DataList
        rows={rows}
        columns={columns}
        rowKey={(h) => h.id}
        isLoading={hooks.isPending}
        error={hooks.error}
        rowActions={(h) => (
          <span className="flex flex-wrap items-center gap-0.5">
            {action("History", () => setLogFor(h))}
            {action("Test", () => test.mutate(h.id), { disabled: test.isPending })}
            {action(h.enabled ? "Pause" : "Resume", () => toggle.mutate(h), { disabled: toggle.isPending })}
            {action("Edit", () => setEditing(h))}
            {action("Rotate secret", () => void askRotate(h))}
            {action("Remove", () => void askDelete(h), { danger: true })}
          </span>
        )}
        empty={
          <EmptyState
            icon={WebhookIcon}
            title="No webhooks yet"
            description="Notify another service — a Slack relay, a static-site rebuild — when your content changes."
            action={<Button size="sm" variant="outline" onClick={() => setCreating(true)}>Add your first webhook</Button>}
          />
        }
      />

      <DeliveryLog hook={logFor} onClose={() => setLogFor(null)} />

      <WebhookForm
        open={creating || editing !== null}
        hook={editing}
        onClose={() => { setCreating(false); setEditing(null); }}
        onSaved={(created) => {
          invalidate();
          if (created) setSecret({ url: created.url, secret: created.secret, header: created.signature_header, rotated: false });
        }}
      />

      <SecretModal info={secret} onClose={() => setSecret(null)} />
    </div>
  );
}

/** The secret, once. Closing it is the last time anyone sees it. */
function SecretModal({ info, onClose }: { info: { url: string; secret: string; header: string; rotated: boolean } | null; onClose: () => void }) {
  const [copied, setCopied] = React.useState(false);
  const copy = () => {
    void navigator.clipboard?.writeText(info?.secret ?? "").then(() => setCopied(true)).catch(() => undefined);
  };
  const snippet = info
    ? `// Node: verify ${info.header}
const [t, v1] = sig.split(",").map((p) => p.split("=")[1]);
const expected = crypto.createHmac("sha256", SECRET).update(\`\${t}.\${raw}\`).digest("hex");
const ok = crypto.timingSafeEqual(Buffer.from(v1, "hex"), Buffer.from(expected, "hex"))
        && Math.abs(Date.now() / 1000 - Number(t)) < 300;`
    : "";
  return (
    <Modal
      open={info !== null}
      onClose={onClose}
      title={info?.rotated ? "New signing secret" : "Your signing secret"}
      description={info?.url}
      size="lg"
      footer={<Button onClick={onClose}>I've saved it</Button>}
    >
      <div className="space-y-3 text-sm" data-testid="webhook-secret">
        <p>Shown once. Store it where the receiver can read it; it is not retrievable later, only rotated.</p>
        <div className="flex items-center gap-2">
          <code className="min-w-0 flex-1 break-all rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs" data-testid="webhook-secret-value">{info?.secret}</code>
          <Button type="button" variant="outline" size="sm" onClick={copy}>{copied ? "Copied" : "Copy"}</Button>
        </div>
        <p className="text-muted-foreground">
          Every delivery carries the header <code className="font-mono text-xs">{info?.header}</code> as <code className="font-mono text-xs">t=&lt;unix seconds&gt;,v1=&lt;hex hmac&gt;</code>, where the HMAC-SHA256 covers <code className="font-mono text-xs">t + "." + raw body</code>. Reject anything older than five minutes.
        </p>
        <pre className="overflow-x-auto rounded-md border bg-muted/40 p-3 text-xs">{snippet}</pre>
      </div>
    </Modal>
  );
}

/** Recent delivery attempts for one webhook, with what came back. */
function DeliveryLog({ hook, onClose }: { hook: Hook | null; onClose: () => void }) {
  const queryClient = useQueryClient();
  const log = useQuery({
    queryKey: ["webhook-deliveries", hook?.id],
    queryFn: () => api.webhookDeliveries(hook?.id as string),
    enabled: hook !== null,
    refetchInterval: hook !== null ? 5000 : false,
  });
  const [open, setOpen] = React.useState<string | null>(null);
  const redeliver = useMutation({
    mutationFn: (d: WebhookDelivery) => api.redeliverWebhook(hook?.id as string, d.id),
    onSuccess: () => {
      notify.success("Queued again", "A new attempt appears here in a few seconds.");
      setTimeout(() => void queryClient.invalidateQueries({ queryKey: ["webhook-deliveries", hook?.id] }), 3000);
    },
    onError: (e) => notify.error("Couldn't redeliver", e),
  });

  const rows = log.data ?? [];
  const failing = rows.filter((d) => d.status !== "success").length;

  return (
    <Modal open={hook !== null} onClose={onClose} title="Delivery history" description={hook?.url} size="lg" footer={<Button variant="outline" onClick={onClose}>Close</Button>}>
      {log.isError ? <ErrorNote title="Couldn't load delivery history" error={log.error} onRetry={() => void log.refetch()} /> : log.isPending ? (
        <Skeleton className="h-32 w-full" />
      ) : rows.length === 0 ? (
        <EmptyState icon={Activity} title="No deliveries yet" description="Attempts appear here once one of the chosen events happens. Send a test to try it now." />
      ) : (
        <div className="space-y-3">
          {failing > 0 ? (
            <p className="rounded-md border border-warning/40 bg-warning-subtle px-3 py-2 text-sm text-warning">{failing} of the last {rows.length} attempts didn&rsquo;t succeed.</p>
          ) : (
            <p className="rounded-md border border-success/40 bg-success-subtle px-3 py-2 text-sm text-success">All {rows.length} recent attempts succeeded.</p>
          )}
          <ul className="divide-y rounded-md border">
            {rows.map((d) => (
              <li key={d.id} className="px-3 py-2 text-sm">
                <div className="flex items-center gap-3">
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-medium">{eventLabel(d.event)}</span>
                    <span className="block text-xs text-muted-foreground">
                      {ago(d.at)} · {d.attempts} {d.attempts === 1 ? "attempt" : "attempts"}
                      {d.response_code === null ? "" : ` · HTTP ${d.response_code}`}
                      {d.error && d.response_code === null ? ` · ${d.error}` : ""}
                    </span>
                  </span>
                  <Chip tone={STATUS_TONE[d.status] ?? "warning"}>{STATUS_LABEL[d.status] ?? d.status}</Chip>
                  {d.response_body || d.payload ? (
                    <button type="button" className="text-xs text-muted-foreground underline-offset-2 hover:underline" onClick={() => setOpen(open === d.id ? null : d.id)}>
                      {open === d.id ? "Hide" : "Details"}
                    </button>
                  ) : null}
                  {d.status !== "success" && d.payload ? (
                    <Button size="sm" variant="outline" disabled={redeliver.isPending} onClick={() => redeliver.mutate(d)}>Redeliver</Button>
                  ) : null}
                </div>
                {open === d.id ? (
                  <div className="mt-2 grid gap-2 text-xs lg:grid-cols-2">
                    <div>
                      <p className="mb-1 font-medium text-muted-foreground">Sent</p>
                      <pre className="max-h-48 overflow-auto rounded-md border bg-muted/40 p-2">{d.payload ?? "(not recorded)"}</pre>
                    </div>
                    <div>
                      <p className="mb-1 font-medium text-muted-foreground">Receiver answered</p>
                      <pre className="max-h-48 overflow-auto rounded-md border bg-muted/40 p-2">{d.response_body ?? d.error ?? "(no body)"}</pre>
                    </div>
                  </div>
                ) : null}
              </li>
            ))}
          </ul>
        </div>
      )}
    </Modal>
  );
}

/** Create and edit share one form; the secret only exists on create. */
function WebhookForm({ open, hook, onClose, onSaved }: { open: boolean; hook: Hook | null; onClose: () => void; onSaved: (created?: WebhookCreated) => void }) {
  const [url, setUrl] = React.useState("");
  const [selected, setSelected] = React.useState<string[]>(["post.published"]);
  React.useEffect(() => {
    if (open) {
      setUrl(hook?.url ?? "");
      setSelected(hook?.events ?? ["post.published"]);
    }
  }, [open, hook]);

  const validUrl = /^https:\/\/.+/.test(url);
  const showUrlError = url !== "" && !validUrl;
  const choosable = EVENTS.filter((e) => e.value !== "test.ping");

  const save = useMutation({
    mutationFn: async () => {
      if (hook) {
        await api.updateWebhook(hook.id, { url, events: selected });
        return undefined;
      }
      return api.createWebhook({ url, events: selected });
    },
    onSuccess: (created) => {
      notify.success(hook ? "Webhook updated" : "Webhook added");
      onSaved(created);
      onClose();
    },
    onError: (e) => notify.error(hook ? "Couldn't update the webhook" : "Couldn't add the webhook", e),
  });

  const toggle = (value: string) => setSelected((prev) => (prev.includes(value) ? prev.filter((v) => v !== value) : [...prev, value]));

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={hook ? "Edit webhook" : "Add a webhook"}
      description={hook ? "The signing secret stays the same; rotate it from the list if the receiver needs a new one." : "We'll POST a signed JSON payload to this address, and retry if it fails."}
      size="lg"
      footer={
        <>
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          <Button disabled={!validUrl || selected.length === 0 || save.isPending} onClick={() => save.mutate()}>
            {save.isPending ? "Saving…" : hook ? "Save changes" : "Add webhook"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label="Endpoint URL" htmlFor="hook-url" hint="Must be https, so the signed payload can't be read in transit." error={showUrlError ? "Enter a full https:// address." : null}>
          <Input id="hook-url" autoFocus value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://example.com/hooks/vyasa" className="font-mono text-xs" />
        </Field>
        <fieldset className="space-y-2">
          <legend className="text-sm font-medium">Send a notification when…</legend>
          <div className="grid gap-1.5 sm:grid-cols-2">
            {choosable.map((e) => (
              <label key={e.value} className="flex cursor-pointer items-center gap-2 rounded-md border px-3 py-2 text-sm hover:bg-accent">
                <input type="checkbox" checked={selected.includes(e.value)} onChange={() => toggle(e.value)} className="h-3.5 w-3.5 accent-primary" />
                <span className="min-w-0 truncate">{e.label}</span>
              </label>
            ))}
          </div>
          {selected.length === 0 ? <p className="text-xs text-destructive">Choose at least one event.</p> : null}
        </fieldset>
      </div>
    </Modal>
  );
}
