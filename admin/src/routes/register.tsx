import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery } from "@tanstack/react-query";
import { api, ApiError } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { Mark } from "@/components/ui/logo";

export const Route = createFileRoute("/register")({ component: RegisterPage });

/**
 * Public sign-up. Always ends at the same "check your email" screen on a
 * well-formed submission, whatever the address turned out to be: the
 * server's `202` is byte-identical for a new address, one that already has
 * an account, and a filled honeypot, so the page must never distinguish
 * them either (that would reveal which addresses have accounts).
 */
function RegisterPage() {
  const info = useQuery({
    queryKey: ["registration-info"],
    queryFn: () => api.registrationInfo(),
    retry: false,
  });
  const [displayName, setDisplayName] = React.useState("");
  const [email, setEmail] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [again, setAgain] = React.useState("");
  // Honeypot: left blank by a person, filled in by most bots that fill
  // every field they find. A filled one gets exactly the same answer as a
  // real submission — nothing here ever treats it differently.
  const [website, setWebsite] = React.useState("");
  const [error, setError] = React.useState<string | null>(null);

  const passwordMin = info.data?.password_min_length ?? 8;
  const mismatch = again !== "" && again !== password;
  const short = password !== "" && password.length < passwordMin;

  const submit = useMutation({
    mutationFn: () =>
      api.register({
        email: email.trim(),
        display_name: displayName.trim() === "" ? null : displayName.trim(),
        password,
        website,
      }),
  });

  function handleSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    if (mismatch || short || email.trim() === "" || password === "") return;
    submit.mutate(undefined, {
      onError: (err) => {
        if (err instanceof ApiError && err.status === 403) {
          setError("This site isn't taking new accounts right now.");
        } else if (err instanceof ApiError && err.status === 503) {
          setError("Registration isn't available right now. Try again later.");
        } else if (err instanceof ApiError && err.status === 429) {
          setError("Too many attempts. Wait a while and try again.");
        } else if (err instanceof ApiError && err.status === 400) {
          setError(err.message);
        } else {
          setError(err.message || "Couldn't reach the server. Check your connection and try again.");
        }
      },
    });
  }

  // Known closed (or on but unable to send the confirmation) before a
  // submission is even tried: skip straight to the message instead of
  // letting someone fill the whole form for a 403 or a 503.
  const closed = !submit.isSuccess && info.data !== undefined && !info.data.available;

  return (
    <div className="grid min-h-dvh place-items-center bg-background px-4 text-foreground">
      <div className="w-full max-w-sm rounded-lg border bg-card p-6" data-testid="register-page">
        <div className="mb-4 flex items-center gap-2 font-serif text-lg font-semibold">
          <Mark className="text-primary" />
          Vyasa
        </div>
        {submit.isSuccess ? (
          <>
            <h1 className="text-lg font-semibold">Check your email</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              If {email.trim()} can be registered, we've sent a message to it. Follow the link in it to finish; it works for 24 hours.
            </p>
            <a href="/admin/login" className="mt-4 inline-block text-sm text-primary underline-offset-2 hover:underline">
              Back to sign in
            </a>
          </>
        ) : closed ? (
          <>
            <h1 className="text-lg font-semibold">Not taking new accounts</h1>
            <p className="mt-2 text-sm text-muted-foreground">This site isn't taking new accounts right now.</p>
            <a href="/admin/login" className="mt-4 inline-block text-sm text-primary underline-offset-2 hover:underline">
              Back to sign in
            </a>
          </>
        ) : (
          <form onSubmit={handleSubmit} className="space-y-4">
            <h1 className="text-lg font-semibold">Create an account</h1>
            <Field label="Display name" htmlFor="register-name">
              <Input id="register-name" autoFocus autoComplete="name" value={displayName} onChange={(e) => setDisplayName(e.target.value)} />
            </Field>
            <Field label="Email address" htmlFor="register-email">
              <Input
                id="register-email"
                type="email"
                required
                autoCapitalize="none"
                spellCheck={false}
                autoComplete="username"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
              />
            </Field>
            <Field
              label="Password"
              htmlFor="register-pw"
              hint={short ? `At least ${passwordMin} characters.` : "Twelve or more is better."}
            >
              <Input
                id="register-pw"
                type="password"
                required
                minLength={passwordMin}
                autoComplete="new-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </Field>
            <Field label="Confirm password" htmlFor="register-pw2" error={mismatch ? "The two passwords differ." : null}>
              <Input
                id="register-pw2"
                type="password"
                required
                autoComplete="new-password"
                value={again}
                onChange={(e) => setAgain(e.target.value)}
              />
            </Field>
            {/*
              Honeypot. A sighted person never sees this field (it sits off
              the visible canvas, not merely `display: none`, which some
              bots skip filling) and assistive tech never announces it
              (`aria-hidden`, out of the tab order). Its name matches the
              API's `website` field exactly; nothing about the markup
              signals "trap" to anything that isn't already ignoring
              `aria-hidden` and tab order.
            */}
            <input
              type="text"
              id="register-website"
              name="website"
              data-testid="register-honeypot"
              aria-hidden="true"
              tabIndex={-1}
              autoComplete="off"
              value={website}
              onChange={(e) => setWebsite(e.target.value)}
              style={{ position: "absolute", left: "-9999px", width: "1px", height: "1px", overflow: "hidden" }}
            />
            {error ? (
              <p role="alert" className="rounded-md border border-destructive/40 bg-destructive-subtle px-3 py-2 text-sm text-destructive">
                {error}
              </p>
            ) : null}
            <Button type="submit" className="w-full" disabled={submit.isPending || email.trim() === "" || password === "" || mismatch || short}>
              {submit.isPending ? "Creating…" : "Create account"}
            </Button>
            <a href="/admin/login" className="block text-center text-sm text-muted-foreground underline-offset-2 hover:underline">
              Already have an account? Sign in
            </a>
          </form>
        )}
      </div>
    </div>
  );
}
