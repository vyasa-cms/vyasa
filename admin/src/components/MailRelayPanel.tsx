import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type MailSettings } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";

const PRESETS: { label: string; host: string; port: number; username?: string; note: string }[] = [
  { label: "Resend", host: "smtp.resend.com", port: 587, username: "resend", note: "password is the API key" },
  { label: "SendGrid", host: "smtp.sendgrid.net", port: 587, username: "apikey", note: "password is the API key" },
  { label: "Postmark", host: "smtp.postmarkapp.com", port: 587, note: "server token as both username and password" },
  { label: "Amazon SES", host: "email-smtp.us-east-1.amazonaws.com", port: 587, note: "SMTP credentials from the SES console; change the region" },
  { label: "Brevo", host: "smtp-relay.brevo.com", port: 587, note: "your login; password is the SMTP key" },
  { label: "Mailgun", host: "smtp.mailgun.org", port: 587, note: "postmaster@your-domain" },
  { label: "Google Workspace", host: "smtp.gmail.com", port: 587, note: "full address; password is an app password" },
];

/**
 * The mail relay, editable in place. Used by Settings and the setup
 * wizard alike. The password is write-only: it is sealed with the server
 * secret on save and never returned, so the field shows only whether
 * one is stored.
 */
export function MailRelayPanel({ testTo, compact = false, onSaved }: { testTo?: string; compact?: boolean; onSaved?: (s: MailSettings) => void }) {
  const queryClient = useQueryClient();
  const current = useQuery({ queryKey: ["mail-settings"], queryFn: () => api.mailSettings(), retry: false });
  const [v, setV] = React.useState({ host: "", port: "587", username: "", password: "", from: "" });
  const [touched, setTouched] = React.useState(false);
  const [to, setTo] = React.useState(testTo ?? "");
  React.useEffect(() => {
    if (current.data && !touched) {
      setV({ host: current.data.host, port: String(current.data.port), username: current.data.username, password: "", from: current.data.from });
    }
  }, [current.data, touched]);
  React.useEffect(() => {
    if (testTo && to === "") setTo(testTo);
  }, [testTo, to]);

  const set = (k: keyof typeof v) => (e: React.ChangeEvent<HTMLInputElement>) => {
    setTouched(true);
    setV({ ...v, [k]: e.target.value });
  };
  const save = useMutation({
    mutationFn: () =>
      api.mailSettingsSave({
        host: v.host,
        port: Number(v.port) || 587,
        username: v.username,
        from: v.from,
        ...(v.password !== "" ? { password: v.password } : {}),
      }),
    onSuccess: (s) => {
      setTouched(false);
      setV((prev) => ({ ...prev, password: "" }));
      queryClient.setQueryData(["mail-settings"], s);
      void queryClient.invalidateQueries({ queryKey: ["setup-checks"] });
      void queryClient.invalidateQueries({ queryKey: ["site-health"] });
      notify.success(s.host === "" ? "Relay removed" : "Relay saved", s.host === "" ? "Mail is logged, not sent." : `${s.host}:${s.port}`);
      onSaved?.(s);
    },
    onError: (e) => notify.error("Couldn't save the relay", e),
  });
  const test = useMutation({
    mutationFn: () => api.mailTest(to.trim()),
    onSuccess: () => notify.success("Sent", `A test message went to ${to.trim()}.`),
    onError: (e) => notify.error("The relay refused it", e),
  });

  const source = current.data?.source ?? "none";
  const status =
    source === "options"
      ? `Saved in the admin panel${current.data?.encrypted ? ", password encrypted" : ", password stored in plain text: set VYASA_SECRET_KEY"}.`
      : source === "environment"
        ? "Set in the environment (VYASA_SMTP__*). Saving here overrides it."
        : "No relay yet. Mail is logged on the server, not sent.";

  return (
    <div className="space-y-4" data-testid="mail-relay">
      <p className="text-sm text-muted-foreground">{status}</p>
      {!compact ? (
        <div className="flex flex-wrap gap-1.5" aria-label="Providers">
          {PRESETS.map((p) => (
            <button
              key={p.label}
              type="button"
              title={p.note}
              onClick={() => {
                setTouched(true);
                setV({ ...v, host: p.host, port: String(p.port), username: p.username ?? v.username });
              }}
              className="rounded-full border px-2.5 py-0.5 text-xs hover:bg-accent"
            >
              {p.label}
            </button>
          ))}
        </div>
      ) : null}
      <div className="grid gap-3 sm:grid-cols-2 2xl:grid-cols-3">
        <Field label="Relay host" htmlFor="smtp-host" hint="Hostname only. Empty removes the relay."><Input id="smtp-host" value={v.host} onChange={set("host")} className="font-mono text-sm" placeholder="smtp.example.com" /></Field>
        <Field label="Port" htmlFor="smtp-port" hint="587 with STARTTLS for nearly every provider."><Input id="smtp-port" value={v.port} onChange={set("port")} inputMode="numeric" className="max-w-[10ch] tabular-nums" /></Field>
        <Field label="From address" htmlFor="smtp-from" hint="An address the provider has verified for your domain."><Input id="smtp-from" type="email" value={v.from} onChange={set("from")} placeholder="hello@yourdomain.com" /></Field>
        <Field label="Username" htmlFor="smtp-user" hint="Empty if the relay needs no login."><Input id="smtp-user" value={v.username} onChange={set("username")} className="font-mono text-sm" autoComplete="off" /></Field>
        <Field label="Password" htmlFor="smtp-pass" hint={current.data?.has_password ? "One is stored. Leave empty to keep it." : "Stored encrypted; never shown again."}>
          <Input id="smtp-pass" type="password" value={v.password} onChange={set("password")} autoComplete="new-password" placeholder={current.data?.has_password ? "••••••••" : ""} />
        </Field>
      </div>
      <div className="flex flex-wrap items-end gap-2">
        <Button type="button" size="sm" disabled={save.isPending || !touched} onClick={() => save.mutate()}>{save.isPending ? "Saving…" : "Save relay"}</Button>
        <span className="flex-1" />
        <div className="min-w-56"><Field label="Send a test to" htmlFor="smtp-test-to"><Input id="smtp-test-to" type="email" value={to} onChange={(e) => setTo(e.target.value)} /></Field></div>
        <Button type="button" size="sm" variant="outline" disabled={test.isPending || to.trim() === "" || touched || source === "none"} title={touched ? "Save the relay first" : undefined} onClick={() => test.mutate()}>
          {test.isPending ? "Sending…" : "Send test"}
        </Button>
      </div>
    </div>
  );
}
