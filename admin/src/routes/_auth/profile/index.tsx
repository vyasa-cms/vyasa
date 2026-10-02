import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "@/api/client";
import { useMe } from "@/components/auth";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, Field, PageHeader, Panel } from "@/components/ui/primitives";
import { MediaPicker } from "@/components/editor/MediaPicker";
import { LOCALES, useI18n } from "@/lib/i18n";
import { notify } from "@/components/ui/toast";

export const Route = createFileRoute("/_auth/profile/")({ component: ProfilePage });

function ago(iso: string | null | undefined): string {
  if (!iso) return "never";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString();
}

/** The signed-in person's own account: name, bio, password, API keys. */
export function ProfilePage() {
  const me = useMe();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const i18n = useI18n();
  const [name, setName] = React.useState("");
  const [bio, setBio] = React.useState("");
  const [pw, setPw] = React.useState({ current: "", next: "", again: "" });
  // Why the server refused a password change (wrong current password,
  // a weak new one), shown at the fields rather than in a passing toast.
  const [pwError, setPwError] = React.useState<string | null>(null);
  const [touched, setTouched] = React.useState(false);
  const [avatar, setAvatar] = React.useState<string | null | undefined>(undefined);
  const [picking, setPicking] = React.useState(false);
  React.useEffect(() => {
    if (me.data && !touched) {
      setName(me.data.display_name);
      setBio(me.data.bio);
    }
  }, [me.data, touched]);

  const save = useMutation({
    // A new password is only accepted with the current one: a borrowed
    // session must not be able to lock the owner out.
    mutationFn: () => api.updateMe({ display_name: name.trim(), bio, ...(pw.next !== "" ? { password: pw.next, current_password: pw.current } : {}), ...(avatar !== undefined ? { avatar_media_id: avatar } : {}) }),
    onMutate: () => setPwError(null),
    onSuccess: () => {
      setTouched(false);
      setAvatar(undefined);
      setPw({ current: "", next: "", again: "" });
      void queryClient.invalidateQueries({ queryKey: ["me"] });
      notify.success("Profile saved");
    },
    onError: (e) => {
      if (pw.next !== "" && e instanceof ApiError && (e.status === 400 || e.status === 403)) {
        setPwError(e.message.startsWith("request failed") ? (e.status === 403 ? "The current password isn't right." : "That password can't be used.") : e.message);
        return;
      }
      notify.error("Couldn't save your profile", e);
    },
  });
  const signOutAll = useMutation({
    mutationFn: () => api.revokeUserSessions(me.data?.id ?? ""),
    onSuccess: () => window.location.assign("/admin/login"),
    onError: (e) => notify.error("Couldn't end your sessions", e),
  });

  const keys = useQuery({ queryKey: ["api-keys"], queryFn: () => api.listApiKeys() });
  const caps = useQuery({ queryKey: ["my-caps"], queryFn: () => api.myCaps(), staleTime: 60_000 });
  const [newKey, setNewKey] = React.useState<{ name: string; key: string } | null>(null);
  const [creating, setCreating] = React.useState(false);
  const [keyName, setKeyName] = React.useState("");
  const [keyCaps, setKeyCaps] = React.useState<string[]>([]);
  const createKey = useMutation({
    mutationFn: () => api.createApiKey({ name: keyName.trim(), capabilities: keyCaps }),
    onSuccess: (k) => {
      setCreating(false);
      setKeyName("");
      setKeyCaps([]);
      setNewKey({ name: k.name, key: k.key });
      void queryClient.invalidateQueries({ queryKey: ["api-keys"] });
    },
    onError: (e) => notify.error("Couldn't create the key", e),
  });
  const revoke = useMutation({
    mutationFn: (id: string) => api.revokeApiKey(id),
    onSuccess: () => { void queryClient.invalidateQueries({ queryKey: ["api-keys"] }); notify.success("Key revoked"); },
    onError: (e) => notify.error("Couldn't revoke the key", e),
  });

  const mismatch = pw.again !== "" && pw.again !== pw.next;
  const short = pw.next !== "" && pw.next.length < 8;

  return (
    <div className="space-y-5">
      <PageHeader title="Your profile" description={me.data ? `${me.data.email} · ${me.data.role_name}` : ""} />
      <div className="grid gap-5 xl:grid-cols-2">
        <Panel title="About you" description="Your byline and the bio themes may show under your posts.">
          <div className="space-y-3">
            <Field label="Avatar" htmlFor="pf-avatar" hint="Shown in the header and on your author page.">
              <div className="flex items-center gap-3">
                {avatar === null ? null : (avatar ? <img src={`/api/v1/media/${avatar}/raw`} alt="" className="h-12 w-12 rounded-full border object-cover" /> : me.data?.avatar_url ? <img src={me.data.avatar_url} alt="" className="h-12 w-12 rounded-full border object-cover" /> : null)}
                <button id="pf-avatar" type="button" className="rounded-md border px-2.5 py-1.5 text-xs font-medium hover:bg-accent" onClick={() => setPicking((v) => !v)}>{picking ? "Close" : "Choose…"}</button>
                {(avatar ?? me.data?.avatar_url) ? <button type="button" className="text-xs text-muted-foreground underline" onClick={() => { setTouched(true); setAvatar(null); }}>Remove</button> : null}
              </div>
              {picking ? <div className="mt-2 rounded-md border p-2"><MediaPicker accept="image" onPick={(m) => { setTouched(true); setAvatar(String(m.id)); setPicking(false); }} /></div> : null}
            </Field>
            <Field label={i18n.t("profile.language")} htmlFor="pf-lang" hint={i18n.t("profile.language_hint")}>
              <select id="pf-lang" value={i18n.locale} onChange={(e) => i18n.setLocale(e.target.value as typeof i18n.locale)} className="h-9 w-full max-w-[16rem] rounded-md border bg-background px-2 text-sm">
                {LOCALES.map((l) => <option key={l.value} value={l.value}>{l.label}</option>)}
              </select>
            </Field>
            <Field label="Display name" htmlFor="pf-name"><Input id="pf-name" value={name} onChange={(e) => { setTouched(true); setName(e.target.value); }} /></Field>
            <Field label="Bio" htmlFor="pf-bio">
              <textarea id="pf-bio" rows={3} value={bio} onChange={(e) => { setTouched(true); setBio(e.target.value); }} className="w-full resize-y rounded-md border border-input bg-background p-2 text-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring" />
            </Field>
            <div className="grid gap-3 sm:grid-cols-3">
              <Field label="Current password" htmlFor="pf-pw0" hint={pw.next !== "" && pw.current === "" ? "Needed to set a new one." : undefined} error={pwError}><Input id="pf-pw0" type="password" autoComplete="current-password" value={pw.current} onChange={(e) => { setPwError(null); setPw({ ...pw, current: e.target.value }); }} /></Field>
              <Field label="New password" htmlFor="pf-pw" hint={short ? "At least 8 characters." : "Leave empty to keep it."}><Input id="pf-pw" type="password" autoComplete="new-password" value={pw.next} onChange={(e) => { setTouched(true); setPw({ ...pw, next: e.target.value }); }} /></Field>
              <Field label="Again" htmlFor="pf-pw2" error={mismatch ? "The two passwords differ." : null}><Input id="pf-pw2" type="password" autoComplete="new-password" value={pw.again} onChange={(e) => setPw({ ...pw, again: e.target.value })} /></Field>
            </div>
            <div className="flex items-center gap-2 border-t pt-3">
              <Button size="sm" disabled={!touched || save.isPending || mismatch || short || (pw.next !== "" && pw.current === "") || name.trim() === ""} onClick={() => save.mutate()}>{save.isPending ? "Saving…" : "Save profile"}</Button>
              <span className="flex-1" />
              <span className="text-xs text-muted-foreground">Last sign-in {ago(me.data?.last_login_at)}</span>
              <Button size="sm" variant="outline" disabled={signOutAll.isPending} onClick={async () => { if (await confirm({ title: "Sign out everywhere?", description: "Every session for your account ends, including this one.", confirmLabel: "Sign out everywhere" })) signOutAll.mutate(); }}>Sign out everywhere</Button>
            </div>
          </div>
        </Panel>

        <TwoFactorPanel />

        <Panel title="API keys" description="For scripts and other services acting as you. A key can hold only capabilities you have." testId="api-keys">
          <div className="space-y-3">
            {(keys.data ?? []).length === 0 ? <p className="text-sm text-muted-foreground">No keys yet.</p> : (
              <ul className="divide-y rounded-md border">
                {(keys.data ?? []).map((k) => (
                  <li key={k.id} className="flex items-center gap-3 px-3 py-2 text-sm">
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-medium">{k.name}</span>
                      <span className="block truncate text-xs text-muted-foreground">{k.capabilities.join(", ") || "no capabilities"} · last used {ago(k.last_used_at)}</span>
                    </span>
                    <Button size="sm" variant="ghost" className="text-destructive" disabled={revoke.isPending} onClick={async () => { if (await confirm({ title: `Revoke "${k.name}"?`, description: "Anything using this key stops working at once.", confirmLabel: "Revoke", destructive: true })) revoke.mutate(k.id); }}>Revoke</Button>
                  </li>
                ))}
              </ul>
            )}
            <Button size="sm" variant="outline" onClick={() => setCreating(true)}>New key</Button>
          </div>
        </Panel>
      </div>

      <Modal open={creating} onClose={() => setCreating(false)} title="New API key" description="Name it after what will use it." footer={<><Button variant="outline" onClick={() => setCreating(false)}>Cancel</Button><Button disabled={keyName.trim() === "" || createKey.isPending} onClick={() => createKey.mutate()}>{createKey.isPending ? "Creating…" : "Create key"}</Button></>}>
        <div className="space-y-3">
          <Field label="Name" htmlFor="key-name"><Input id="key-name" autoFocus value={keyName} onChange={(e) => setKeyName(e.target.value)} placeholder="Deploy script" /></Field>
          <fieldset>
            <legend className="text-sm font-medium">Capabilities</legend>
            <div className="mt-1 grid gap-1 sm:grid-cols-2">
              {(caps.data ?? []).map((c) => (
                <label key={c} className="flex items-center gap-2 text-sm"><input type="checkbox" className="accent-primary" checked={keyCaps.includes(c)} onChange={() => setKeyCaps((prev) => (prev.includes(c) ? prev.filter((x) => x !== c) : [...prev, c]))} />{c}</label>
              ))}
            </div>
          </fieldset>
        </div>
      </Modal>

      <Modal open={newKey !== null} onClose={() => setNewKey(null)} title="Your new key" description={newKey?.name} footer={<Button onClick={() => setNewKey(null)}>I've saved it</Button>}>
        <p className="text-sm">Shown once. Send it as <code className="font-mono text-xs">Authorization: Bearer …</code>.</p>
        <code className="mt-2 block break-all rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs" data-testid="new-api-key">{newKey?.key}</code>
        <Chip tone="warning" dot={false}>Not retrievable later</Chip>
      </Modal>
    </div>
  );
}


/** Time-based codes: set up with a QR, confirm with the first code, keep the recovery codes. */
function TwoFactorPanel() {
  const queryClient = useQueryClient();
  const status = useQuery({ queryKey: ["mfa-status"], queryFn: () => api.mfaStatus() });
  const [setup, setSetup] = React.useState<{ otpauth_url: string; qr_svg: string; secret: string } | null>(null);
  const [code, setCode] = React.useState("");
  const [codes, setCodes] = React.useState<string[] | null>(null);
  const [offCode, setOffCode] = React.useState("");
  const refresh = () => void queryClient.invalidateQueries({ queryKey: ["mfa-status"] });
  const begin = useMutation({ mutationFn: () => api.mfaSetup(), onSuccess: setSetup, onError: (e) => notify.error("Couldn't start setup", e) });
  const confirm = useMutation({
    mutationFn: () => api.mfaConfirm(code.trim()),
    onSuccess: (r) => { setCodes(r.codes); setSetup(null); setCode(""); refresh(); notify.success("Second factor on", "Keep the recovery codes somewhere safe."); },
    onError: (e) => notify.error("That code didn't work", e),
  });
  const disable = useMutation({
    mutationFn: () => api.mfaDisable(offCode.trim()),
    onSuccess: () => { setOffCode(""); setCodes(null); refresh(); notify.success("Second factor off"); },
    onError: (e) => notify.error("That code didn't work", e),
  });
  const on = status.data?.enabled ?? false;
  return (
    <Panel title="Two-factor sign-in" description="A code from an authenticator app is asked for after the password. Any app that speaks TOTP works: Google Authenticator, Authy, 1Password, Bitwarden." testId="two-factor">
      <div className="space-y-3 text-sm">
        {on ? (
          <>
            <p><Chip tone="success" dot={false}>On</Chip> <span className="ml-2 text-muted-foreground">{status.data?.recovery_codes_left ?? 0} recovery codes left.</span></p>
            <div className="flex flex-wrap items-end gap-2">
              <Field label="Turn off with a current code" htmlFor="mfa-off"><Input id="mfa-off" inputMode="numeric" value={offCode} onChange={(e) => setOffCode(e.target.value)} className="max-w-[12ch] font-mono" /></Field>
              <Button size="sm" variant="outline" disabled={offCode.trim() === "" || disable.isPending} onClick={() => disable.mutate()}>Turn off</Button>
            </div>
          </>
        ) : setup ? (
          <div className="grid gap-4 sm:grid-cols-[auto_1fr]" data-testid="mfa-setup">
            <div className="rounded-md border bg-white p-2" dangerouslySetInnerHTML={{ __html: setup.qr_svg }} />
            <div className="space-y-2">
              <p>Scan the code with your authenticator app, or type this secret into it:</p>
              <code className="block break-all rounded-md border bg-muted/40 px-2 py-1 font-mono text-xs" data-testid="mfa-secret">{setup.secret}</code>
              <div className="flex flex-wrap items-end gap-2">
                <Field label="Then enter the code it shows" htmlFor="mfa-code"><Input id="mfa-code" inputMode="numeric" autoComplete="one-time-code" value={code} onChange={(e) => setCode(e.target.value)} className="max-w-[12ch] font-mono" /></Field>
                <Button size="sm" disabled={code.trim() === "" || confirm.isPending} onClick={() => confirm.mutate()}>{confirm.isPending ? "Checking…" : "Turn on"}</Button>
                <Button size="sm" variant="ghost" onClick={() => setSetup(null)}>Cancel</Button>
              </div>
            </div>
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-3">
            <Chip tone="neutral" dot={false}>Off</Chip>
            <Button size="sm" variant="outline" disabled={begin.isPending} onClick={() => begin.mutate()}>Set up</Button>
          </div>
        )}
        {codes ? (
          <div className="rounded-md border border-warning/40 bg-warning-subtle p-3" data-testid="recovery-codes">
            <p className="font-medium">Recovery codes, shown once</p>
            <p className="text-xs text-muted-foreground">Each works one time if the phone is gone. Store them with your passwords.</p>
            <ul className="mt-2 grid grid-cols-2 gap-1 font-mono text-xs sm:grid-cols-4">{codes.map((c) => <li key={c}>{c}</li>)}</ul>
            <Button size="sm" variant="ghost" className="mt-2" onClick={() => setCodes(null)}>I've saved them</Button>
          </div>
        ) : null}
      </div>
    </Panel>
  );
}
