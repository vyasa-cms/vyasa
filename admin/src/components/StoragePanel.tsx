import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type StorageInput, type StorageSettings } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { useConfirm } from "@/components/ui/dialog";

const PRESETS: { label: string; endpoint: string; region: string; path_style: boolean; note: string }[] = [
  { label: "Cloudflare R2", endpoint: "https://<account-id>.r2.cloudflarestorage.com", region: "auto", path_style: true, note: "R2 → Manage API tokens → Object Read & Write for the bucket" },
  { label: "AWS S3", endpoint: "https://s3.us-east-1.amazonaws.com", region: "us-east-1", path_style: false, note: "an IAM user with access to the bucket; change the region" },
  { label: "MinIO", endpoint: "http://minio:9000", region: "us-east-1", path_style: true, note: "the root user or a service account" },
];

type Form = { provider: "local" | "s3"; bucket: string; region: string; endpoint: string; path_style: boolean; access_key_id: string; secret_access_key: string };

function fromSettings(s: StorageSettings): Form {
  return { provider: s.provider, bucket: s.bucket, region: s.region, endpoint: s.endpoint, path_style: s.path_style, access_key_id: "", secret_access_key: "" };
}

function toInput(v: Form, forget = false): StorageInput {
  if (v.provider === "local") return { provider: "local" };
  return {
    provider: "s3",
    bucket: v.bucket.trim(),
    region: v.region.trim(),
    endpoint: v.endpoint.trim(),
    path_style: v.path_style,
    ...(v.access_key_id !== "" ? { access_key_id: v.access_key_id } : {}),
    ...(v.secret_access_key !== "" ? { secret_access_key: v.secret_access_key } : {}),
    ...(forget ? { forget_existing: true } : {}),
  };
}

/**
 * Where media bytes live, editable in place. The keys are write-only:
 * sealed with the server secret on save and never returned, so the
 * fields show only whether one is stored. Files already uploaded stay
 * where they are after a switch; "Move existing files" copies them.
 */
export function StoragePanel() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const current = useQuery({
    queryKey: ["storage-settings"],
    queryFn: () => api.storageSettings(),
    retry: false,
    refetchInterval: (q) => (q.state.data?.migration?.state === "running" ? 1500 : false),
  });
  const [v, setV] = React.useState<Form>({ provider: "local", bucket: "", region: "auto", endpoint: "", path_style: true, access_key_id: "", secret_access_key: "" });
  const [touched, setTouched] = React.useState(false);
  React.useEffect(() => {
    if (current.data && !touched) setV(fromSettings(current.data));
  }, [current.data, touched]);

  const set = (k: keyof Form) => (e: React.ChangeEvent<HTMLInputElement>) => {
    setTouched(true);
    setV({ ...v, [k]: e.target.type === "checkbox" ? e.target.checked : e.target.value });
  };
  const setProvider = (provider: Form["provider"]) => {
    setTouched(true);
    setV({ ...v, provider });
  };

  const applied = (s: StorageSettings) => {
    setTouched(false);
    setV(fromSettings(s));
    queryClient.setQueryData(["storage-settings"], s);
    void queryClient.invalidateQueries({ queryKey: ["site-health"] });
  };
  const save = useMutation({
    mutationFn: (forget: boolean) => api.storageSettingsSave(toInput(v, forget)),
    onSuccess: (s) => {
      applied(s);
      notify.success(s.provider === "s3" ? "Object storage active" : "Local disk active", s.provider === "s3" ? `New uploads go to ${s.bucket}.` : "New uploads go to the server's disk.");
    },
    onError: async (e: unknown) => {
      const msg = e instanceof Error ? e.message : String(e);
      if (/forgotten/.test(msg)) {
        const ok = await confirm({ title: "Files are in the current bucket", description: `${msg}. Forget them? They will stop serving until you point back at that bucket.`, confirmLabel: "Forget and switch", destructive: true });
        if (ok) save.mutate(true);
        return;
      }
      notify.error("Couldn't save storage settings", e);
    },
  });
  const test = useMutation({
    mutationFn: () => api.storageTest(toInput(v)),
    onSuccess: () => notify.success("Connected", "Wrote, read back and deleted a probe object."),
    onError: (e) => notify.error("The store refused", e),
  });
  const migrate = useMutation({
    mutationFn: () => api.storageMigrate(),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["storage-settings"] });
      notify.success("Moving files", "Progress updates below.");
    },
    onError: (e) => notify.error("Couldn't start the move", e),
  });

  const data = current.data;
  const source = data?.source ?? "none";
  const locked = source === "environment";
  const status = locked
    ? "Set by the operator in the environment (VYASA_STORAGE__*); change it there."
    : source === "options"
      ? `Saved in the admin panel${data?.encrypted ? ", keys encrypted" : ", keys stored in plain text: set VYASA_SECRET_KEY"}.`
      : "Uploads go to the server's local disk.";
  const other = data ? (data.provider === "s3" ? data.counts.local : data.counts.s3) : 0;
  const migration = data?.migration ?? null;
  const running = migration?.state === "running";
  // A move needs an object store on the server: the active one, or the
  // saved one when uploads went back to local disk.
  const canMove = !!data && other > 0 && (data.provider === "s3" || data.has_secret) && !data.keys_unreadable;
  const s3Form = v.provider === "s3";
  const canTest = s3Form && v.bucket.trim() !== "" && v.endpoint.trim() !== "" && (v.access_key_id !== "" || data?.has_secret) && (v.secret_access_key !== "" || data?.has_secret);

  return (
    <div className="space-y-4" data-testid="storage-panel">
      <p className="text-sm text-muted-foreground">{status}</p>
      {data?.keys_unreadable ? (
        <p className="text-xs text-destructive" data-testid="storage-keys-unreadable">The stored keys can no longer be read (the server secret changed). Uploads go to local disk until you enter the access key id and secret again and save.</p>
      ) : null}
      {data?.ephemeral_disk && data.provider === "local" ? (
        <p className="text-xs text-warning" data-testid="storage-ephemeral">This server's disk does not survive a restart: uploads will be lost. Point media at a bucket.</p>
      ) : null}
      <div className="flex flex-wrap gap-4" role="radiogroup" aria-label="Where uploads go">
        {(["local", "s3"] as const).map((p) => (
          <label key={p} className="flex items-center gap-2 text-sm">
            <input type="radio" name="storage-provider" value={p} checked={v.provider === p} disabled={locked} onChange={() => setProvider(p)} />
            {p === "local" ? "Local disk" : "Object storage (S3-compatible)"}
          </label>
        ))}
      </div>
      {s3Form ? (
        <>
          {!locked ? (
            <div className="flex flex-wrap gap-1.5" aria-label="Providers">
              {PRESETS.map((p) => (
                <button key={p.label} type="button" title={p.note} onClick={() => { setTouched(true); setV({ ...v, endpoint: p.endpoint, region: p.region, path_style: p.path_style }); }} className="rounded-full border px-2.5 py-0.5 text-xs hover:bg-accent">
                  {p.label}
                </button>
              ))}
            </div>
          ) : null}
          <div className="grid gap-3 sm:grid-cols-2 2xl:grid-cols-3">
            <Field label="Bucket" htmlFor="storage-bucket"><Input id="storage-bucket" value={v.bucket} onChange={set("bucket")} disabled={locked} className="font-mono text-sm" placeholder="my-site-media" /></Field>
            <Field label="Endpoint" htmlFor="storage-endpoint" hint="The service URL, not the bucket's."><Input id="storage-endpoint" value={v.endpoint} onChange={set("endpoint")} disabled={locked} className="font-mono text-sm" placeholder="https://…" /></Field>
            <Field label="Region" htmlFor="storage-region" hint="auto for R2 and most MinIO setups."><Input id="storage-region" value={v.region} onChange={set("region")} disabled={locked} className="font-mono text-sm max-w-[16ch]" /></Field>
            <Field label="Access key id" htmlFor="storage-key" hint={data?.access_key_id_hint ? `One ending in ${data.access_key_id_hint} is stored. Leave empty to keep it.` : "From the provider's API token."}>
              <Input id="storage-key" value={v.access_key_id} onChange={set("access_key_id")} disabled={locked} className="font-mono text-sm" autoComplete="off" placeholder={data?.access_key_id_hint ? `…${data.access_key_id_hint}` : ""} />
            </Field>
            <Field label="Secret access key" htmlFor="storage-secret" hint={data?.has_secret ? "One is stored. Leave empty to keep it." : "Stored encrypted; never shown again."}>
              <Input id="storage-secret" type="password" value={v.secret_access_key} onChange={set("secret_access_key")} disabled={locked} autoComplete="new-password" placeholder={data?.has_secret ? "••••••••" : ""} />
            </Field>
            <label className="flex items-center gap-2 self-end pb-2 text-sm">
              <input type="checkbox" checked={v.path_style} disabled={locked} onChange={set("path_style")} />
              Path-style addressing
            </label>
          </div>
        </>
      ) : null}
      {!locked ? (
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" disabled={save.isPending || !touched || running} title={running ? "Wait for the move to finish" : undefined} onClick={() => save.mutate(false)}>{save.isPending ? "Saving…" : "Save storage"}</Button>
          {s3Form ? (
            <Button type="button" size="sm" variant="outline" disabled={test.isPending || !canTest} onClick={() => test.mutate()}>{test.isPending ? "Testing…" : "Test connection"}</Button>
          ) : null}
        </div>
      ) : null}
      {data ? (
        <div className="space-y-2 border-t pt-3" data-testid="storage-counts">
          <p className="text-xs text-muted-foreground">
            {data.counts.local} {data.counts.local === 1 ? "file" : "files"} on local disk, {data.counts.s3} in object storage.
          </p>
          {canMove ? (
            <div className="flex flex-wrap items-center gap-2">
              <Button type="button" size="sm" variant="outline" disabled={running || migrate.isPending || touched} title={touched ? "Save the settings first" : undefined} onClick={() => migrate.mutate()}>
                {running ? "Moving…" : `Move ${other} ${other === 1 ? "file" : "files"} to ${data.provider === "s3" ? "object storage" : "local disk"}`}
              </Button>
            </div>
          ) : null}
          {migration ? (
            <p className="text-xs text-muted-foreground" data-testid="storage-migration">
              {migration.state === "running" ? `Moving: ${migration.done} of ${migration.total} done` : migration.state === "done" ? `Last move: ${migration.done} moved.` : `Last move: ${migration.done} moved, ${migration.failed} failed.`}
              {migration.last_error ? ` ${migration.last_error}` : ""}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
