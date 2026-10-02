import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { Plug, Upload } from "lucide-react";

import { api, type PluginInspection, type PluginSurface } from "@/api/client";
import {
  capabilityLabel,
  deletePlugin,
  inspectPlugin,
  installPlugin,
  listPlugins,
  pluginAudit,
  rollbackPlugin,
  setPluginEnabled,
  type PluginSummary,
} from "@/api/plugins";
import { PluginSurfacePanels, SettingsForm } from "@/components/plugins/surface";
import { Button } from "@/components/ui/button";
import { DataList, type Column } from "@/components/ui/data-list";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, EmptyState, PageHeader, Panel } from "@/components/ui/primitives";
import { RegistryBrowser } from "@/components/RegistryBrowser";
import { notify } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/plugins/")({
  component: PluginsPage,
});

/** Capabilities that hand a plugin reach beyond its own sandbox. */
const SENSITIVE = ["net:", "db:write", "fs:", "secrets:"];
const MAX_MB = 10;

function capabilityTone(cap: string): "warning" | "neutral" {
  return SENSITIVE.some((p) => cap.startsWith(p)) ? "warning" : "neutral";
}

const STATUS_LABEL: Record<string, string> = { installed: "Installed", loaded: "Running", degraded: "Degraded", errored: "Failed", disabled: "Disabled" };
const STATUS_TONE: Record<string, "success" | "warning" | "danger" | "neutral"> = { installed: "neutral", loaded: "success", degraded: "warning", errored: "danger", disabled: "neutral" };

/**
 * Whether a plugin's status reason is a clash with an administrator's
 * content type: refused at enable ("…is a content type created by an
 * administrator…") or, at start-up, its type skipped ("…not loaded: an
 * administrator's content type holds the slug").
 */
function isTypeClash(reason: string | undefined): boolean {
  return reason !== undefined && /content type created by an administrator|an administrator's content type holds the slug/.test(reason);
}

/** What a slug clash means and what to do about it, in the admin's terms. */
function TypeClashNote({ plugin }: { plugin: PluginSummary }) {
  return (
    <p className="text-[11px] text-muted-foreground" data-testid="type-clash-note">
      {plugin.enabled
        ? "It is running without that type: a content type made under "
        : "It was turned off: a content type made under "}
      <Link to="/content-types" className="underline">Content types</Link>
      {plugin.enabled
        ? " already uses the slug. Saving its settings or upgrading it keeps it on, still without that type. Turning it off and on again, or rolling it back, is refused while the clash lasts: delete that content type (once it has no entries) or use a version of the plugin with another slug."
        : " already uses the slug. Delete that content type (once it has no entries) or use a version of the plugin with another slug, then enable it again."}
    </p>
  );
}

export function PluginsPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const plugins = useQuery({ queryKey: ["plugins"], queryFn: listPlugins });
  // The marketplace index, for "update available" on installed rows.
  const registry = useQuery({ queryKey: ["registry", "plugin", ""], queryFn: () => api.browseRegistry({ kind: "plugin" }), retry: false });
  const [detail, setDetail] = React.useState<PluginSummary | null>(null);

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["plugins"] });
    void queryClient.invalidateQueries({ queryKey: ["plugin-surface"] });
  };

  const toggle = useMutation({
    mutationFn: (p: PluginSummary) => setPluginEnabled(p.id, !p.enabled),
    onSuccess: (_r, p) => { invalidate(); notify.success(p.enabled ? `${p.name} disabled` : `${p.name} enabled`); },
    onError: (e) => notify.error("Couldn't change the plugin", e),
  });
  const rollback = useMutation({
    mutationFn: ({ p, version }: { p: PluginSummary; version?: string }) => rollbackPlugin(p.id, version),
    onSuccess: (_r, { p, version }) => { invalidate(); notify.success(version ? `${p.name} switched to ${version}` : `${p.name} rolled back to its previous version`); },
    onError: (e) => notify.error("Rollback failed", e),
  });
  const remove = useMutation({
    mutationFn: (p: PluginSummary) => deletePlugin(p.id),
    onSuccess: (_r, p) => { invalidate(); setDetail(null); notify.success(`${p.name} removed`); },
    onError: (e) => notify.error("Couldn't remove the plugin", e),
  });

  const askRollback = async (p: PluginSummary) => {
    const ok = await confirm({ title: `Roll ${p.name} back?`, description: `Version ${p.version} is replaced by the previously installed version. Its settings are kept.`, confirmLabel: "Roll back" });
    if (ok) rollback.mutate({ p });
  };
  const askDelete = async (p: PluginSummary) => {
    const ok = await confirm({ title: `Remove ${p.name}?`, description: "The plugin and its stored settings are deleted. Anything it added to your site stops working. Disabling keeps everything and just switches it off.", confirmLabel: "Remove plugin", destructive: true, requireTyped: p.name });
    if (ok) remove.mutate(p);
  };

  const rows = plugins.data ?? [];
  const latestOf = (name: string) => registry.data?.entries?.find((e) => e.name === name)?.versions?.[0]?.version ?? null;
  const updateFor = (p: PluginSummary) => { const l = latestOf(p.name); return l && l !== p.version ? l : null; };
  const priorVersions = (p: PluginSummary) => (p.versions ?? []).filter((v) => v !== p.version);

  const columns: Column<PluginSummary>[] = [
    {
      key: "name",
      header: "Plugin",
      primary: true,
      render: (p) => (
        <span className="min-w-0">
          <span className="flex flex-wrap items-center gap-1.5 font-medium">
            <button type="button" className="truncate underline-offset-2 hover:underline" onClick={() => setDetail(p)}>{p.name}</button>
            <span className="text-[11px] font-normal text-muted-foreground">{p.version}{p.author ? ` · ${p.author}` : ""}</span>
            {updateFor(p) ? <Chip tone="warning" dot={false}>{updateFor(p)} available</Chip> : null}
          </span>
          {p.description ? <span className="block truncate text-[11px] text-muted-foreground">{p.description}</span> : null}
        </span>
      ),
    },
    {
      key: "status",
      header: "Status",
      width: "12rem",
      render: (p) => (
        <span data-testid={`plugin-status-${p.name}`} className="min-w-0">
          <Chip tone={p.enabled ? (STATUS_TONE[p.status] ?? "neutral") : "neutral"}>{p.enabled ? (STATUS_LABEL[p.status] ?? p.status) : "Disabled"}</Chip>
          {(p.enabled || isTypeClash(p.status_reason)) && p.status_reason ? <span className="mt-0.5 block truncate text-[11px] text-destructive" title={p.status_reason}>{p.status_reason}</span> : null}
          {isTypeClash(p.status_reason) ? <TypeClashNote plugin={p} /> : null}
        </span>
      ),
    },
    {
      key: "capabilities",
      header: "Can access",
      width: "minmax(0,1.4fr)",
      hideBelow: "lg",
      render: (p) => (
        <span className="flex min-w-0 flex-wrap gap-1">
          {p.capabilities.length === 0 ? <span className="text-xs text-muted-foreground">Nothing outside itself</span> : p.capabilities.map((c) => <Chip key={c} tone={capabilityTone(c)} dot={false}>{capabilityLabel(c)}</Chip>)}
        </span>
      ),
    },
  ];

  const action = (label: string, onClick: () => void, opts: { disabled?: boolean; danger?: boolean } = {}) => (
    <button type="button" onClick={onClick} disabled={opts.disabled} className={cn("rounded-md px-2 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50", opts.danger ? "text-destructive hover:bg-destructive-subtle" : "text-muted-foreground hover:text-foreground")}>{label}</button>
  );

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader title="Plugins" description="Extra features, each running in a sandbox with only the access it declares." />

      <DataList
        testId="plugins-table"
        rowTestId={(p) => `plugin-row-${p.name}`}
        rows={rows}
        columns={columns}
        rowKey={(p) => p.id}
        isLoading={plugins.isPending}
        error={plugins.error}
        rowActions={(p) => (
          <span className="flex flex-wrap items-center gap-0.5">
            {action("Details", () => setDetail(p))}
            <Button size="sm" variant="outline" className="h-7 text-xs" onClick={() => toggle.mutate(p)}>{p.enabled ? "Disable" : "Enable"}</Button>
            {priorVersions(p).length > 0 ? action("Roll back", () => void askRollback(p), { disabled: rollback.isPending }) : null}
            {action("Remove", () => void askDelete(p), { danger: true })}
          </span>
        )}
        empty={<EmptyState testId="plugins-empty" icon={Plug} title="No plugins installed" description="Install one from the marketplace or upload a package below." />}
      />

      <div className="grid gap-4 2xl:grid-cols-2">
        <InstallSection onInstalled={invalidate} />
        <RegistryBrowser kind="plugin" />
      </div>

      <PluginSurfacePanels />

      {detail ? <PluginDetail plugin={rows.find((p) => p.id === detail.id) ?? detail} onClose={() => setDetail(null)} onRollback={(version) => rollback.mutate({ p: detail, version })} onRemove={() => void askDelete(detail)} /> : null}
    </div>
  );
}

/** Everything about one plugin: metadata, versions, settings, what it adds, audit. */
function PluginDetail({ plugin, onClose, onRollback, onRemove }: { plugin: PluginSummary; onClose: () => void; onRollback: (version: string) => void; onRemove: () => void }) {
  const audit = useQuery({ queryKey: ["plugin-audit", plugin.id], queryFn: () => pluginAudit(plugin.id) });
  const surface = useQuery<PluginSurface>({ queryKey: ["plugin-surface"], queryFn: () => api.getPluginSurface() });
  const s = surface.data;
  const mine = <T extends { pluginId: string }>(xs: T[] | undefined) => (xs ?? []).filter((x) => x.pluginId === plugin.id);
  const forms = mine(s?.forms);
  const blocks = mine(s?.blocks);
  const routes = mine(s?.routes);
  const counts = plugin.audit ?? {};
  return (
    <Modal open onClose={onClose} size="lg" title={plugin.name} description={`${plugin.version}${plugin.author ? ` · ${plugin.author}` : ""}${plugin.license ? ` · ${plugin.license}` : ""}`} testId="plugin-detail"
      footer={<><Button variant="ghost" className="text-destructive" onClick={onRemove}>Remove</Button><span className="flex-1" /><Button variant="outline" onClick={onClose}>Close</Button></>}>
      <div className="space-y-4 text-sm">
        {plugin.description ? <p>{plugin.description}</p> : null}
        {plugin.homepage ? <a href={plugin.homepage} target="_blank" rel="noreferrer" className="text-primary underline-offset-2 hover:underline">{plugin.homepage}</a> : null}
        {plugin.status_reason ? <p className="rounded-md border border-warning/40 bg-warning-subtle px-3 py-2 text-warning">{plugin.status_reason}</p> : null}
        {isTypeClash(plugin.status_reason) ? <TypeClashNote plugin={plugin} /> : null}

        <section>
          <h3 className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">Access</h3>
          <p className="flex flex-wrap gap-1">{plugin.capabilities.length === 0 ? <span className="text-muted-foreground">Nothing outside itself.</span> : plugin.capabilities.map((c) => <Chip key={c} tone={capabilityTone(c)} dot={false}>{capabilityLabel(c)}</Chip>)}</p>
        </section>

        <section>
          <h3 className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">Versions</h3>
          <ul className="divide-y rounded-md border" data-testid="plugin-versions">
            {(plugin.versions ?? [plugin.version]).map((v) => (
              <li key={v} className="flex items-center gap-2 px-3 py-1.5">
                <span className="font-mono text-xs">{v}</span>
                {v === plugin.version ? <Chip tone="success" dot={false}>active</Chip> : <Button size="sm" variant="ghost" className="ml-auto h-7 text-xs" onClick={() => onRollback(v)}>Switch to this</Button>}
              </li>
            ))}
          </ul>
          <p className="mt-1 text-xs text-muted-foreground">Uploading a newer package upgrades in place; older versions stay here to switch back to.</p>
        </section>

        {forms.map((f) => <SettingsForm key={f.pluginId} form={f} />)}

        {blocks.length > 0 || routes.length > 0 ? (
          <section>
            <h3 className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">Adds to the site</h3>
            <p className="flex flex-wrap gap-1">
              {blocks.map((b) => <Chip key={b.kind} dot={false}>{b.icon === null ? "" : `${b.icon} `}{b.title} <span className="ml-1 text-muted-foreground">block</span></Chip>)}
              {routes.map((r) => <Chip key={`${r.method} ${r.path}`} dot={false}><code>{r.method} {r.path}</code></Chip>)}
            </p>
          </section>
        ) : null}

        <section>
          <h3 className="mb-1 text-xs font-semibold uppercase tracking-wide text-muted-foreground">Activity</h3>
          <p className="text-xs text-muted-foreground">{counts["deny"] ?? 0} denied · {counts["fetch"] ?? 0} fetches · {counts["quota"] ?? 0} quota hits</p>
          {(audit.data ?? []).length === 0 ? <p className="mt-1 text-xs text-muted-foreground">Nothing recorded yet.</p> : (
            <ul className="mt-1 max-h-48 divide-y overflow-y-auto rounded-md border text-xs" data-testid="plugin-audit">
              {(audit.data ?? []).map((row, i) => (
                <li key={i} className="flex gap-2 px-3 py-1">
                  <span className="shrink-0 text-muted-foreground">{new Date(row.ts).toLocaleString()}</span>
                  <Chip tone={row.kind === "deny" ? "warning" : "neutral"} dot={false}>{row.kind}</Chip>
                  <span className="min-w-0 truncate font-mono">{row.capability}</span>
                  <span className="min-w-0 truncate text-muted-foreground">{row.detail}</span>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </Modal>
  );
}

/** Upload: drop the package, read what it is, then install. */
function InstallSection({ onInstalled }: { onInstalled: () => void }) {
  const [file, setFile] = React.useState<File | null>(null);
  const [seen, setSeen] = React.useState<PluginInspection | null>(null);
  const [over, setOver] = React.useState(false);

  const inspect = useMutation({
    mutationFn: (f: File) => inspectPlugin(f),
    onSuccess: setSeen,
    onError: (e) => { setSeen(null); notify.error("That isn't a valid plugin package", e); },
  });
  const install = useMutation({
    mutationFn: () => { if (file === null) return Promise.reject(new Error("choose a file first")); return installPlugin(file); },
    onSuccess: (r) => {
      setFile(null);
      setSeen(null);
      // An upgrade whose declarations clash with a content type stays on,
      // without the clashing part.
      if (r.status === "degraded") notify.info(`${r.name} ${r.version} installed, but not all of it is loaded`, "The plugin list says what was skipped and why.");
      else notify.success("Plugin installed", "Enable it when you're ready.");
      onInstalled();
    },
    onError: (e: Error) => notify.error("Install failed", e),
  });

  const choose = (f: File | null) => {
    setFile(f);
    setSeen(null);
    if (f) {
      if (f.size > MAX_MB * 1024 * 1024) { notify.error("Package too large", `The limit is ${MAX_MB} MB.`); setFile(null); return; }
      inspect.mutate(f);
    }
  };

  return (
    <Panel testId="install-section" title="Upload a plugin" description={`A signed .vyplugin package, up to ${MAX_MB} MB. Uploading a newer version of an installed plugin upgrades it.`}>
      <div className="space-y-3">
        <label
          className={cn("flex cursor-pointer flex-col items-center gap-1 rounded-md border border-dashed px-3 py-6 text-sm", over && "border-primary bg-primary/5")}
          onDragOver={(e) => { e.preventDefault(); setOver(true); }}
          onDragLeave={() => setOver(false)}
          onDrop={(e) => { e.preventDefault(); setOver(false); choose(e.dataTransfer.files?.[0] ?? null); }}
        >
          <Upload className="h-5 w-5 text-muted-foreground" aria-hidden="true" />
          <span>{file ? file.name : "Drop a .vyplugin here, or choose a file"}</span>
          <input type="file" accept=".vyplugin,application/zip" aria-label="plugin file" onChange={(e) => choose(e.target.files?.[0] ?? null)} className="sr-only" />
        </label>

        {inspect.isPending ? <p className="text-sm text-muted-foreground">Reading the package…</p> : null}

        {seen ? (
          <div className="space-y-2 rounded-md border p-3 text-sm" data-testid="inspection">
            <div className="flex flex-wrap items-baseline gap-x-2">
              <span className="font-medium">{seen.name}</span>
              <span className="text-xs text-muted-foreground">{seen.version}{seen.author ? ` · ${seen.author}` : ""}{seen.license ? ` · ${seen.license}` : ""} · {(seen.wasm_bytes / 1024).toFixed(0)} KB</span>
              {seen.installed_version ? <Chip tone="neutral" dot={false}>installed {seen.installed_version}{seen.installed_version === seen.version ? " (same version)" : " → upgrade"}</Chip> : null}
            </div>
            {seen.description ? <p className="text-muted-foreground">{seen.description}</p> : null}
            <p className="flex flex-wrap gap-1">
              {seen.capabilities.length === 0 ? <span className="text-xs text-muted-foreground">Requests no capabilities.</span> : seen.capabilities.map((c) => <Chip key={c} tone={seen.new_capabilities.includes(c) ? "warning" : capabilityTone(c)} dot={false}>{capabilityLabel(c)}{seen.new_capabilities.includes(c) && seen.installed_version ? " (new)" : ""}</Chip>)}
            </p>
            {seen.trusted ? (
              <p className="text-xs text-success" data-testid="signature-ok">Signed by a trusted key.</p>
            ) : (
              <p className="text-xs text-destructive" data-testid="signature-bad">
                Not signed by any trusted key (signature {seen.signature_prefix}…). Add the author's public key under <a href="/admin/settings#settings-updates" className="underline">Settings → Marketplace and updates</a>, then upload again.
              </p>
            )}
          </div>
        ) : null}

        <Button disabled={file === null || !seen || !seen.trusted || install.isPending} onClick={() => install.mutate()} data-testid="install-button">
          {install.isPending ? "Installing…" : seen?.installed_version && seen.installed_version !== seen.version ? "Upgrade" : "Install"}
        </Button>
      </div>
    </Panel>
  );
}
