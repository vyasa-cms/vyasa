import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation } from "@tanstack/react-query";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { Mark } from "@/components/ui/logo";
import { notify } from "@/components/ui/toast";

export const Route = createFileRoute("/reset")({
  validateSearch: (s: Record<string, unknown>): { token?: string; welcome?: string } => ({
    ...(typeof s["token"] === "string" ? { token: s["token"] } : {}),
    ...(typeof s["welcome"] === "string" ? { welcome: s["welcome"] } : {}),
  }),
  component: ResetPage,
});

/** The landing page for both reset links and invitations. */
function ResetPage() {
  const { token, welcome } = Route.useSearch();
  const [password, setPassword] = React.useState("");
  const [again, setAgain] = React.useState("");
  const save = useMutation({
    mutationFn: () => api.resetPassword(token ?? "", password),
    onError: (e) => notify.error("That link didn't work", e),
  });
  const mismatch = again !== "" && again !== password;
  const short = password !== "" && password.length < 8;
  return (
    <div className="grid min-h-dvh place-items-center bg-background px-4 text-foreground">
      <div className="w-full max-w-sm rounded-lg border bg-card p-6" data-testid="reset-page">
        <div className="mb-4 flex items-center gap-2 font-serif text-lg font-semibold"><Mark className="text-primary" />Vyasa</div>
        {!token ? (
          <>
            <h1 className="text-lg font-semibold">This link is incomplete</h1>
            <p className="mt-2 text-sm text-muted-foreground">Open the link from the email again, or <a href="/admin/forgot" className="text-primary underline-offset-2 hover:underline">request a new one</a>.</p>
          </>
        ) : save.isSuccess ? (
          <>
            <h1 className="text-lg font-semibold">{welcome ? "You're all set" : "Password changed"}</h1>
            <p className="mt-2 text-sm text-muted-foreground">Every other session for this account has been signed out. Sign in with your new password.</p>
            <Button className="mt-4 w-full" onClick={() => window.location.assign("/admin/login")}>Sign in</Button>
          </>
        ) : (
          <form onSubmit={(e) => { e.preventDefault(); if (!mismatch && !short) save.mutate(); }} className="space-y-4">
            <h1 className="text-lg font-semibold">{welcome ? "Welcome. Choose a password" : "Choose a new password"}</h1>
            <Field label="New password" htmlFor="reset-pw" hint={short ? "At least 8 characters." : "Twelve or more is better."}>
              <Input id="reset-pw" type="password" required autoFocus minLength={8} value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" />
            </Field>
            <Field label="Again" htmlFor="reset-pw2" error={mismatch ? "The two passwords differ." : null}>
              <Input id="reset-pw2" type="password" required value={again} onChange={(e) => setAgain(e.target.value)} autoComplete="new-password" />
            </Field>
            <Button type="submit" className="w-full" disabled={save.isPending || password === "" || mismatch || short}>{save.isPending ? "Saving…" : "Set password"}</Button>
          </form>
        )}
      </div>
    </div>
  );
}
