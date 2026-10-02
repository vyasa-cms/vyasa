import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, ChevronDown, ChevronUp, KeyRound, Plus, Search, Star, Zap } from "lucide-react";
import { api, type AiCatalogEntry, type AiKind, type AiModel, type AiProvider, type AiRegistry } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, EmptyState, Field, PageHeader, Panel, Skeleton } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { formatRelative } from "@/routes/_auth/index";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/models/")({
  component: ModelsPage,
});

const KEY = ["ai", "models"];

/** Which Settings switch each kind of job feeds, for the link on the panel. */
const SETTINGS_ANCHOR = "/admin/settings#settings-assistants";

const usd = (n: number) => (n < 0.01 && n > 0 ? "<$0.01" : `$${n.toFixed(2)}`);

export function ModelsPage() {
  const registry = useQuery({ queryKey: KEY, queryFn: () => api.aiModels() });
  const [adding, setAdding] = React.useState<{ kind?: string; provider?: string } | null>(null);
  const [editing, setEditing] = React.useState<AiModel | null>(null);
  const [showEmpty, setShowEmpty] = React.useState(false);

  const data = registry.data;
  const kindsWithModels = data ? data.kinds.filter((k) => data.models.some((m) => m.kind === k.kind)) : [];
  const emptyKinds = data ? data.kinds.filter((k) => !data.models.some((m) => m.kind === k.kind)) : [];

  return (
    <div className="space-y-6">
      <PageHeader
        title="AI models"
        description="Register the providers and models the site may call, and pick a default for each kind of job."
        actions={
          <Button size="sm" onClick={() => setAdding({})} data-testid="add-model">
            <Plus className="h-4 w-4" aria-hidden="true" />
            Add model
          </Button>
        }
      />

      {registry.isPending ? (
        <div className="space-y-3"><Skeleton className="h-32" /><Skeleton className="h-48" /></div>
      ) : registry.isError || !data ? (
        <EmptyState icon={AlertTriangle} title="Couldn't load the registry" description={String((registry.error as Error)?.message ?? "")} />
      ) : (
        <>
          {!data.encrypting_keys ? (
            <p className="flex items-start gap-2 rounded-lg border border-warning/40 bg-warning-subtle px-3 py-2 text-sm" role="status" data-testid="plain-keys-note">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-warning" aria-hidden="true" />
              <span>
                Provider keys are stored <strong>unencrypted</strong>. Set <code className="rounded bg-muted px-1">VYASA_SECRET_KEY</code> on the server to seal them at rest (existing keys are re-sealed the next time they are saved).
              </span>
            </p>
          ) : null}

          {data.spend ? <SpendPanel spend={data.spend} models={data.models} /> : null}

          <section className="space-y-3" aria-labelledby="providers-heading">
            <h2 id="providers-heading" className="text-sm font-semibold">Providers</h2>
            <div className="grid gap-3 md:grid-cols-2 2xl:grid-cols-3">
              {data.providers.map((p) => <ProviderCard key={p.provider} provider={p} onAddModel={() => setAdding({ provider: p.provider })} />)}
            </div>
          </section>

          <section className="space-y-4" aria-labelledby="models-heading">
            <div className="flex items-baseline justify-between">
              <h2 id="models-heading" className="text-sm font-semibold">Models by job</h2>
              <a href={SETTINGS_ANCHOR} className="text-xs text-primary underline-offset-2 hover:underline">Which features use them: Settings → Assistants</a>
            </div>
            {kindsWithModels.length === 0 ? (
              <p className="text-sm text-muted-foreground">No models yet. Add a key to a provider above, then register a model for each job you want the site to do.</p>
            ) : null}
            {kindsWithModels.map((kind) => (
              <KindPanel key={kind.kind} kind={kind} models={data.models.filter((m) => m.kind === kind.kind)} providers={data.providers} onAdd={() => setAdding({ kind: kind.kind })} onEdit={setEditing} />
            ))}
            {emptyKinds.length > 0 ? (
              <div className="rounded-lg border bg-card/60 p-3 text-sm" data-testid="empty-kinds">
                <button type="button" className="flex w-full items-center gap-2 text-left" onClick={() => setShowEmpty((v) => !v)} aria-expanded={showEmpty}>
                  {showEmpty ? <ChevronUp className="h-4 w-4" aria-hidden="true" /> : <ChevronDown className="h-4 w-4" aria-hidden="true" />}
                  <span className="font-medium">{emptyKinds.length} {emptyKinds.length === 1 ? "job has" : "jobs have"} no model yet</span>
                  <span className="text-muted-foreground">{emptyKinds.map((k) => k.label).join(", ")}</span>
                </button>
                {showEmpty ? (
                  <ul className="mt-3 grid gap-2 sm:grid-cols-2 2xl:grid-cols-3">
                    {emptyKinds.map((k) => (
                      <li key={k.kind} className="flex items-start justify-between gap-2 rounded-md border px-3 py-2">
                        <span><span className="block font-medium">{k.label}</span><span className="block text-xs text-muted-foreground">{k.used_by}</span></span>
                        <Button variant="outline" size="sm" onClick={() => setAdding({ kind: k.kind })}><Plus className="h-3.5 w-3.5" aria-hidden="true" />Add</Button>
                      </li>
                    ))}
                  </ul>
                ) : null}
              </div>
            ) : null}
          </section>
        </>
      )}

      {adding !== null && data !== undefined ? <AddModelDialog registry={data} initialKind={adding.kind} initialProvider={adding.provider} onClose={() => setAdding(null)} /> : null}
      {editing !== null ? <EditModelDialog model={editing} onClose={() => setEditing(null)} /> : null}
    </div>
  );
}

/* ---------------------------------------------------------------- spend */

function SpendPanel({ spend, models }: { spend: NonNullable<AiRegistry["spend"]>; models: AiModel[] }) {
  const cap = spend.cap_usd;
  const pct = cap && cap > 0 ? Math.min(100, (spend.total_usd / cap) * 100) : null;
  const purposes = Object.entries(spend.by_purpose).sort((a, b) => b[1] - a[1]);
  const calls = models.reduce((n, m) => n + (m.month_calls ?? 0), 0);
  return (
    <div className="grid gap-3 rounded-lg border bg-card p-4 md:grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)]" data-testid="ai-spend">
      <div>
        <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">This month</p>
        <p className="mt-1 text-2xl font-semibold tabular-nums">{usd(spend.total_usd)}<span className="ml-2 text-sm font-normal text-muted-foreground">{cap === null ? "no cap" : `of ${usd(cap)} cap`}</span></p>
        {pct !== null ? (
          <div className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-muted" aria-hidden="true">
            <div className={cn("h-full rounded-full", pct >= 90 ? "bg-destructive" : pct >= 70 ? "bg-warning" : "bg-primary")} style={{ width: `${pct}%` }} />
          </div>
        ) : null}
        <p className="mt-2 text-xs text-muted-foreground">
          {calls} {calls === 1 ? "call" : "calls"} across {models.length} {models.length === 1 ? "model" : "models"}. Cap and switches live in <a href={SETTINGS_ANCHOR} className="text-primary underline-offset-2 hover:underline">Settings → Assistants</a>.
        </p>
      </div>
      <div>
        <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">By purpose</p>
        {purposes.length === 0 ? <p className="mt-1 text-sm text-muted-foreground">Nothing spent yet.</p> : (
          <ul className="mt-1 grid gap-x-6 gap-y-0.5 text-sm sm:grid-cols-2">
            {purposes.map(([k, v]) => <li key={k} className="flex justify-between gap-3"><span className="truncate text-muted-foreground">{k}</span><span className="tabular-nums">{usd(v)}</span></li>)}
          </ul>
        )}
      </div>
    </div>
  );
}

/* ------------------------------------------------------------ providers */

function ProviderCard({ provider, onAddModel }: { provider: AiProvider; onAddModel: () => void }) {
  const client = useQueryClient();
  const confirm = useConfirm();
  const [open, setOpen] = React.useState(false);
  const [key, setKey] = React.useState("");
  const [baseUrl, setBaseUrl] = React.useState(provider.base_url);
  const invalidate = () => void client.invalidateQueries({ queryKey: KEY });
  React.useEffect(() => setBaseUrl(provider.base_url), [provider.base_url]);

  const save = useMutation({
    mutationFn: (body: { api_key?: string; base_url?: string; enabled?: boolean }) =>
      api.saveAiProvider(provider.provider, { base_url: baseUrl, enabled: provider.enabled, ...body }),
    onSuccess: () => { setKey(""); setOpen(false); invalidate(); notify.success(`${provider.label} saved`); },
    onError: (e) => notify.error(`Couldn't save ${provider.label}`, e),
  });
  const test = useMutation({
    mutationFn: () => api.testAiProvider(provider.provider),
    onSuccess: (r) => notify.success(`${provider.label} works`, r.detail),
    onError: (e) => notify.error(`${provider.label} test failed`, e),
  });
  const remove = useMutation({
    mutationFn: () => api.deleteAiProvider(provider.provider),
    onSuccess: () => { invalidate(); notify.success(`${provider.label} removed`); },
    onError: (e) => notify.error(`Couldn't remove ${provider.label}`, e),
  });
  const askRemove = async () => {
    const ok = await confirm({ title: `Remove ${provider.label}?`, description: "Its key and every model registered at it are removed.", confirmLabel: "Remove", destructive: true });
    if (ok) remove.mutate();
  };

  const status = provider.configured
    ? provider.key_source === "environment"
      ? { tone: "success" as const, text: "Key from environment" }
      : provider.key_source === "none"
        ? { tone: "success" as const, text: "No key needed" }
        : { tone: "success" as const, text: provider.key_sealed ? "Key stored, encrypted" : "Key stored" }
    : { tone: "neutral" as const, text: provider.needs_key ? "No key" : "No base URL" };
  const showForm = open || !provider.configured;
  const canSave = key.trim() !== "" || baseUrl !== provider.base_url;

  return (
    <div className={cn("space-y-3 rounded-lg border bg-card p-4", !provider.enabled && "opacity-70")} data-testid={`provider-${provider.provider}`}>
      <div className="flex items-center gap-2">
        <KeyRound className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
        <h3 className="text-sm font-semibold">{provider.label}</h3>
        <Chip tone={status.tone} className="ml-auto">{status.text}</Chip>
      </div>
      <p className="text-xs text-muted-foreground">
        {provider.supports.map((k) => k.replace("_", " ")).join(" · ")}
        {provider.key_hint !== "" ? ` · key ${provider.key_hint}` : ""}
        {provider.base_url !== "" ? ` · ${provider.base_url}` : ""}
      </p>

      {showForm ? (
        <form className="space-y-2" onSubmit={(e) => { e.preventDefault(); save.mutate(key.trim() === "" ? {} : { api_key: key.trim() }); }}>
          {provider.needs_key || provider.key_source === "stored" || key !== "" ? (
            <Field label={provider.configured ? "Replace API key" : "API key"} htmlFor={`key-${provider.provider}`} hint={provider.needs_key ? undefined : "Optional for a local server."}>
              <Input id={`key-${provider.provider}`} type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)} placeholder={provider.provider === "anthropic" ? "sk-ant-…" : "sk-…"} className="font-mono text-xs" />
            </Field>
          ) : (
            <Field label="API key" htmlFor={`key-${provider.provider}`} hint="Optional for a local server.">
              <Input id={`key-${provider.provider}`} type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)} className="font-mono text-xs" />
            </Field>
          )}
          <Field label="Base URL" hint={provider.needs_base_url ? "Required. Ollama: http://127.0.0.1:11434/v1 · LM Studio: http://127.0.0.1:1234/v1" : `Default: ${provider.default_base_url}`} htmlFor={`url-${provider.provider}`}>
            <Input id={`url-${provider.provider}`} value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder={provider.default_base_url} className="font-mono text-xs" />
          </Field>
          <div className="flex flex-wrap items-center gap-2">
            <Button type="submit" size="sm" disabled={save.isPending || !canSave}>Save</Button>
            {provider.configured ? <Button type="button" variant="ghost" size="sm" onClick={() => { setOpen(false); setKey(""); setBaseUrl(provider.base_url); }}>Cancel</Button> : null}
          </div>
        </form>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" variant="outline" size="sm" disabled={test.isPending} onClick={() => test.mutate()}>
            <Zap className="h-3.5 w-3.5" aria-hidden="true" />{test.isPending ? "Testing…" : "Test"}
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={onAddModel}><Plus className="h-3.5 w-3.5" aria-hidden="true" />Add model</Button>
          <Button type="button" variant="ghost" size="sm" onClick={() => setOpen(true)}>Change key</Button>
          <label className="ml-auto inline-flex items-center gap-1.5 text-xs">
            <input type="checkbox" checked={provider.enabled} onChange={(e) => save.mutate({ enabled: e.target.checked })} aria-label={`${provider.label} enabled`} />
            Enabled
          </label>
          {provider.key_source === "stored" || (!provider.needs_key && provider.configured) ? (
            <Button type="button" variant="ghost" size="sm" className="h-8 px-2 text-destructive" onClick={() => void askRemove()}>Remove</Button>
          ) : provider.key_source === "environment" ? (
            <span className="text-[11px] text-muted-foreground" title="The key comes from the server environment; unset the variable to remove it.">Set by environment</span>
          ) : null}
        </div>
      )}
      {!provider.enabled ? <p className="text-xs text-warning">Disabled: its models are skipped until you enable it.</p> : null}
    </div>
  );
}

/* --------------------------------------------------------------- models */

function KindPanel({ kind, models, providers, onAdd, onEdit }: { kind: AiKind; models: AiModel[]; providers: AiProvider[]; onAdd: () => void; onEdit: (m: AiModel) => void }) {
  const client = useQueryClient();
  const confirm = useConfirm();
  const invalidate = () => void client.invalidateQueries({ queryKey: KEY });

  const setDefault = useMutation({
    mutationFn: (id: string) => api.defaultAiModel(id),
    onSuccess: (m) => { invalidate(); notify.success(`${m.label} is now the default for ${kind.label.toLowerCase()}`); },
    onError: (e) => notify.error("Couldn't change the default", e),
  });
  const toggle = useMutation({
    mutationFn: (m: AiModel) => api.editAiModel(m.id, { enabled: !m.enabled }),
    onSuccess: invalidate,
    onError: (e) => notify.error("Couldn't update the model", e),
  });
  const test = useMutation({
    mutationFn: (id: string) => api.testAiModel(id),
    onSuccess: (r) => { invalidate(); if (r.ok) notify.success("Model works", r.detail); else notify.error("Model test failed", r.detail); },
    onError: (e) => notify.error("Model test failed", e),
  });
  const remove = useMutation({
    mutationFn: (id: string) => api.deleteAiModel(id),
    onSuccess: () => { invalidate(); notify.success("Model removed"); },
    onError: (e) => notify.error("Couldn't remove the model", e),
  });
  const order = useMutation({
    mutationFn: (ids: string[]) => api.orderAiModels(kind.kind, ids),
    onSuccess: invalidate,
    onError: (e) => notify.error("Couldn't reorder", e),
  });

  const askRemove = async (m: AiModel) => {
    const ok = await confirm({ title: `Remove ${m.label}?`, description: m.is_default ? "It is the default for this job; the next in order takes over, if there is one." : "Features that use this job will no longer fall back to it.", confirmLabel: "Remove", destructive: true });
    if (ok) remove.mutate(m.id);
  };
  const providerLabel = (id: string) => providers.find((p) => p.provider === id)?.label ?? id;

  // Order as the server tries them: default first, then sort order.
  const ordered = [...models].sort((a, b) => Number(b.is_default) - Number(a.is_default) || (a.sort_order ?? 0) - (b.sort_order ?? 0) || a.created_at.localeCompare(b.created_at));
  const move = (index: number, dir: -1 | 1) => {
    const next = [...ordered];
    const j = index + dir;
    if (j < 0 || j >= next.length) return;
    [next[index], next[j]] = [next[j]!, next[index]!];
    order.mutate(next.map((m) => m.id));
  };

  return (
    <Panel title={kind.label} description={kind.used_by}>
      <div className="mb-2 flex justify-end"><Button variant="outline" size="sm" onClick={onAdd}><Plus className="h-3.5 w-3.5" aria-hidden="true" />Add</Button></div>
      <div className="overflow-x-auto">
        <table className="w-full text-sm" data-testid={`models-${kind.kind}`}>
          <thead>
            <tr className="text-left text-[11px] uppercase tracking-wide text-muted-foreground">
              <th className="w-10 pb-1 font-medium">#</th>
              <th className="pb-1 font-medium">Model</th>
              <th className="pb-1 font-medium">Status</th>
              <th className="pb-1 font-medium">Cost / Mtok</th>
              <th className="pb-1 font-medium">This month</th>
              <th className="pb-1 font-medium">Last test</th>
              <th className="pb-1 text-right font-medium">Actions</th>
            </tr>
          </thead>
          <tbody className="divide-y">
            {ordered.map((m, i) => {
              const off = !m.enabled || m.provider_enabled === false;
              return (
                <tr key={m.id} className={cn(off && "opacity-60")}>
                  <td className="py-2 pr-2 align-top">
                    <span className="flex flex-col items-center">
                      <button type="button" aria-label={`Move ${m.label} up`} disabled={i === 0 || order.isPending} onClick={() => move(i, -1)} className="text-muted-foreground hover:text-foreground disabled:opacity-30"><ChevronUp className="h-3.5 w-3.5" aria-hidden="true" /></button>
                      <span className="tabular-nums text-xs">{i + 1}</span>
                      <button type="button" aria-label={`Move ${m.label} down`} disabled={i === ordered.length - 1 || order.isPending} onClick={() => move(i, 1)} className="text-muted-foreground hover:text-foreground disabled:opacity-30"><ChevronDown className="h-3.5 w-3.5" aria-hidden="true" /></button>
                    </span>
                  </td>
                  <td className="py-2 pr-3 align-top">
                    <span className="flex flex-wrap items-center gap-1.5 font-medium">
                      {m.label}
                      {m.is_default ? <Chip tone="success" dot={false}><Star className="h-3 w-3" aria-hidden="true" /> Default</Chip> : null}
                    </span>
                    <span className="block truncate font-mono text-xs text-muted-foreground">{providerLabel(m.provider)} · {m.model}</span>
                  </td>
                  <td className="py-2 pr-3 align-top">
                    <span className="flex flex-wrap gap-1">
                      {!m.enabled ? <Chip tone="neutral">Disabled</Chip> : m.provider_enabled === false ? <Chip tone="warning">Provider off</Chip> : <Chip tone="success" dot={false}>On</Chip>}
                      {m.breaker_open ? <Chip tone="danger">Tripped, retrying in a minute</Chip> : null}
                    </span>
                  </td>
                  <td className="py-2 pr-3 align-top text-xs tabular-nums text-muted-foreground">
                    {m.input_cost_per_mtok !== null || m.output_cost_per_mtok !== null ? `$${m.input_cost_per_mtok ?? "?"} in / $${m.output_cost_per_mtok ?? "?"} out` : "not set"}
                  </td>
                  <td className="py-2 pr-3 align-top text-xs tabular-nums text-muted-foreground">
                    {(m.month_calls ?? 0) > 0 ? `${m.month_calls} calls · ${usd(m.month_cost_usd ?? 0)}` : "no calls"}
                  </td>
                  <td className="py-2 pr-3 align-top text-xs">
                    {m.last_probe_ok === null ? <span className="text-muted-foreground">Not tested yet</span> : m.last_probe_ok ? <span className="text-success">{m.last_probe_detail} · {formatRelative(m.last_probe_at ?? "")}</span> : <span className="text-destructive">{m.last_probe_detail}</span>}
                  </td>
                  <td className="py-2 align-top">
                    <span className="flex flex-wrap justify-end gap-0.5">
                      <Button variant="outline" size="sm" className="h-7 text-xs" disabled={test.isPending} onClick={() => test.mutate(m.id)}><Zap className="h-3 w-3" aria-hidden="true" />Test</Button>
                      {!m.is_default ? <Button variant="ghost" size="sm" className="h-7 text-xs" onClick={() => setDefault.mutate(m.id)}>Make default</Button> : null}
                      <Button variant="ghost" size="sm" className="h-7 text-xs" onClick={() => onEdit(m)}>Edit</Button>
                      <Button variant="ghost" size="sm" className="h-7 text-xs" onClick={() => toggle.mutate(m)}>{m.enabled ? "Disable" : "Enable"}</Button>
                      <Button variant="ghost" size="sm" className="h-7 px-2 text-xs text-destructive" aria-label={`Remove ${m.label}`} onClick={() => void askRemove(m)}>Remove</Button>
                    </span>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </Panel>
  );
}

/* ------------------------------------------------------------ edit dialog */

function EditModelDialog({ model, onClose }: { model: AiModel; onClose: () => void }) {
  const client = useQueryClient();
  const s = model.settings;
  const [label, setLabel] = React.useState(model.label);
  const [inCost, setInCost] = React.useState(model.input_cost_per_mtok === null ? "" : String(model.input_cost_per_mtok));
  const [outCost, setOutCost] = React.useState(model.output_cost_per_mtok === null ? "" : String(model.output_cost_per_mtok));
  const [maxTokens, setMaxTokens] = React.useState(typeof s["max_tokens"] === "number" ? String(s["max_tokens"]) : "");
  const [temperature, setTemperature] = React.useState(typeof s["temperature"] === "number" ? String(s["temperature"]) : "");
  const [dimensions, setDimensions] = React.useState(typeof s["dimensions"] === "number" ? String(s["dimensions"]) : "");
  const [size, setSize] = React.useState(typeof s["size"] === "string" ? s["size"] : "");
  const chat = model.kind === "text" || model.kind === "vision";

  const save = useMutation({
    mutationFn: () => {
      const settings: Record<string, unknown> = { ...s };
      const put = (k: string, v: string, num: boolean) => { if (v.trim() === "") delete settings[k]; else settings[k] = num ? Number(v) : v.trim(); };
      put("max_tokens", maxTokens, true);
      put("temperature", temperature, true);
      put("dimensions", dimensions, true);
      put("size", size, false);
      return api.editAiModel(model.id, { label: label.trim() || model.model, settings, input_cost_per_mtok: inCost === "" ? null : Number(inCost), output_cost_per_mtok: outCost === "" ? null : Number(outCost) });
    },
    onSuccess: () => { void client.invalidateQueries({ queryKey: KEY }); notify.success("Model updated"); onClose(); },
    onError: (e) => notify.error("Couldn't update the model", e),
  });

  return (
    <Modal open onClose={onClose} size="md" title={`Edit ${model.label}`} description={`${model.provider} · ${model.model}`} testId="edit-model-dialog"
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={() => save.mutate()} disabled={save.isPending}>{save.isPending ? "Saving…" : "Save"}</Button></>}>
      <div className="space-y-3">
        <Field label="Label" htmlFor="em-label"><Input id="em-label" value={label} onChange={(e) => setLabel(e.target.value)} /></Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Input cost, $ per Mtok" htmlFor="em-in"><Input id="em-in" type="number" step="0.01" min="0" value={inCost} onChange={(e) => setInCost(e.target.value)} placeholder="unknown" /></Field>
          <Field label="Output cost, $ per Mtok" htmlFor="em-out"><Input id="em-out" type="number" step="0.01" min="0" value={outCost} onChange={(e) => setOutCost(e.target.value)} placeholder="unknown" /></Field>
        </div>
        {chat ? (
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Max output tokens" htmlFor="em-max" hint="Empty uses the built-in ceiling (8192)."><Input id="em-max" type="number" min="1" step="1" value={maxTokens} onChange={(e) => setMaxTokens(e.target.value)} /></Field>
            <Field label="Temperature" htmlFor="em-temp" hint="0 to 2; empty uses the provider default."><Input id="em-temp" type="number" min="0" max="2" step="0.1" value={temperature} onChange={(e) => setTemperature(e.target.value)} /></Field>
          </div>
        ) : null}
        {model.kind === "embedding" ? <Field label="Dimensions" htmlFor="em-dim" hint="Only for models that accept a reduced size."><Input id="em-dim" type="number" min="1" step="1" value={dimensions} onChange={(e) => setDimensions(e.target.value)} /></Field> : null}
        {model.kind === "image" ? <Field label="Image size" htmlFor="em-size" hint="e.g. 1024x1024"><Input id="em-size" value={size} onChange={(e) => setSize(e.target.value)} /></Field> : null}
      </div>
    </Modal>
  );
}

/* ----------------------------------------------------------- add dialog */

function AddModelDialog({ registry, initialKind, initialProvider, onClose }: { registry: AiRegistry; initialKind?: string; initialProvider?: string; onClose: () => void }) {
  const client = useQueryClient();
  const configured = registry.providers.filter((p) => p.configured && p.enabled);
  const [provider, setProvider] = React.useState(initialProvider ?? configured[0]?.provider ?? registry.providers[0]?.provider ?? "openai");
  const [kind, setKind] = React.useState(initialKind ?? "text");
  const [model, setModel] = React.useState("");
  const [label, setLabel] = React.useState("");
  const [inCost, setInCost] = React.useState("");
  const [outCost, setOutCost] = React.useState("");
  const [makeDefault, setMakeDefault] = React.useState(false);
  const [browse, setBrowse] = React.useState("");
  const [browsing, setBrowsing] = React.useState(false);

  const current = registry.providers.find((p) => p.provider === provider);
  const supportedKinds = registry.kinds.filter((k) => current?.supports.includes(k.kind) ?? true);
  React.useEffect(() => {
    if (!supportedKinds.some((k) => k.kind === kind) && supportedKinds[0] !== undefined) setKind(supportedKinds[0].kind);
  }, [supportedKinds, kind]);
  const rec = current?.recommended[kind];
  const duplicate = registry.models.some((m) => m.provider === provider && m.kind === kind && m.model === model.trim());

  const catalog = useQuery({
    queryKey: ["ai", "catalog", provider, kind, browse],
    queryFn: () => api.aiCatalog(provider, browse, kind),
    enabled: browsing && (current?.configured ?? false),
    staleTime: 60_000,
    retry: false,
  });

  const register = useMutation({
    mutationFn: () => api.registerAiModel({ provider, kind, model: model.trim(), label: label.trim(), input_cost_per_mtok: inCost === "" ? null : Number(inCost), output_cost_per_mtok: outCost === "" ? null : Number(outCost), make_default: makeDefault }),
    onSuccess: (m) => { void client.invalidateQueries({ queryKey: KEY }); notify.success(`${m.label} registered`, m.is_default ? "It is the default for this job." : undefined); onClose(); },
    onError: (e) => notify.error("Couldn't register the model", e),
  });

  const pick = (entry: AiCatalogEntry) => { setModel(entry.id); if (label.trim() === "") setLabel(entry.name); setBrowsing(false); };
  const useRecommended = () => {
    if (!rec) return;
    setModel(rec.model);
    setLabel(rec.label);
    setInCost(rec.input_cost_per_mtok === null ? "" : String(rec.input_cost_per_mtok));
    setOutCost(rec.output_cost_per_mtok === null ? "" : String(rec.output_cost_per_mtok));
  };

  return (
    <Modal open onClose={onClose} size="md" title="Add a model" description="One provider model id, registered for one kind of job." testId="add-model-dialog"
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button onClick={() => register.mutate()} disabled={register.isPending || model.trim() === "" || duplicate} data-testid="register-model">{register.isPending ? "Registering…" : "Register"}</Button></>}>
      <div className="space-y-3">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Provider" htmlFor="am-provider">
            <select id="am-provider" value={provider} onChange={(e) => setProvider(e.target.value)} className="h-9 w-full rounded-md border bg-background px-2 text-sm">
              {registry.providers.map((p) => <option key={p.provider} value={p.provider}>{p.label}{p.configured ? "" : p.needs_key ? " (no key yet)" : " (no base URL yet)"}</option>)}
            </select>
          </Field>
          <Field label="Job" htmlFor="am-kind">
            <select id="am-kind" value={kind} onChange={(e) => setKind(e.target.value)} className="h-9 w-full rounded-md border bg-background px-2 text-sm">
              {supportedKinds.map((k) => <option key={k.kind} value={k.kind}>{k.label}</option>)}
            </select>
          </Field>
        </div>

        <Field label="Model id" hint={current?.configured ? "Type it, or browse what your key can use." : "Configure this provider first to browse its catalogue."} htmlFor="am-model" error={duplicate ? "That model is already registered for this job." : null}>
          <div className="flex gap-2">
            <Input id="am-model" value={model} onChange={(e) => setModel(e.target.value)} placeholder={rec?.model ?? "model id"} className="font-mono text-xs" data-testid="model-id" />
            <Button type="button" variant="outline" size="sm" className="h-9 shrink-0" disabled={!current?.configured} onClick={() => setBrowsing((v) => !v)} aria-pressed={browsing}><Search className="h-3.5 w-3.5" aria-hidden="true" />Browse</Button>
          </div>
        </Field>
        {rec ? (
          <button type="button" className="text-xs text-primary underline-offset-2 hover:underline" onClick={useRecommended} data-testid="use-recommended">
            Use the recommended {rec.label} ({rec.model})
          </button>
        ) : null}

        {browsing ? (
          <div className="rounded-lg border bg-muted/30 p-2" data-testid="catalog">
            <Input autoFocus value={browse} onChange={(e) => setBrowse(e.target.value)} placeholder="Filter the catalogue" aria-label="Filter the catalogue" className="mb-2 h-8" />
            {catalog.isPending ? <p className="px-1 py-2 text-xs text-muted-foreground">Asking {current?.label}…</p>
              : catalog.isError ? <p className="px-1 py-2 text-xs text-destructive">{(catalog.error as Error).message}</p>
              : catalog.data.length === 0 ? <p className="px-1 py-2 text-xs text-muted-foreground">Nothing matches.</p>
              : (
                <ul className="max-h-48 overflow-y-auto">
                  {catalog.data.map((entry) => (
                    <li key={entry.id}>
                      <button type="button" onClick={() => pick(entry)} className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs hover:bg-accent">
                        <span className="min-w-0 flex-1 truncate font-mono">{entry.id}</span>
                        <span className="truncate text-muted-foreground">{entry.name !== entry.id ? entry.name : ""}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
          </div>
        ) : null}

        <Field label="Label" hint="How it appears in menus. Defaults to the id." htmlFor="am-label"><Input id="am-label" value={label} onChange={(e) => setLabel(e.target.value)} /></Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Input cost, $ per Mtok" htmlFor="am-in"><Input id="am-in" type="number" step="0.01" min="0" value={inCost} onChange={(e) => setInCost(e.target.value)} placeholder="optional" /></Field>
          <Field label="Output cost, $ per Mtok" htmlFor="am-out"><Input id="am-out" type="number" step="0.01" min="0" value={outCost} onChange={(e) => setOutCost(e.target.value)} placeholder="optional" /></Field>
        </div>
        <label className="inline-flex items-center gap-2 text-sm"><input type="checkbox" checked={makeDefault} onChange={(e) => setMakeDefault(e.target.checked)} />Make it the default for this job</label>
      </div>
    </Modal>
  );
}
