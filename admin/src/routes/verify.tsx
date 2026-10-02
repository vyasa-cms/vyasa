import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation } from "@tanstack/react-query";
import { api, ApiError } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { Mark } from "@/components/ui/logo";

export const Route = createFileRoute("/verify")({
  validateSearch: (s: Record<string, unknown>): { token?: string } =>
    typeof s["token"] === "string" ? { token: s["token"] } : {},
  component: VerifyPage,
});

/**
 * The landing page for a confirmation link. It never posts on arrival:
 * opening a link proves only that something reached the mailbox (mail
 * scanners follow links), and anyone can register anyone's address. So
 * the person confirms deliberately, either with the password they chose
 * when they signed up (which is then kept) or without it, in which case
 * the server removes any stored password and mails a link to set one.
 * Someone who did not sign up is told to close the page (the mail says
 * the same): confirming is never their business.
 *
 * A missing, bad or expired token gets one failure screen, with a resend
 * form so a dead link isn't a dead end, rather than a distinct message for
 * each: the token alone can't say *why* it failed.
 */
function VerifyPage() {
  const { token } = Route.useSearch();
  const [password, setPassword] = React.useState("");
  const [email, setEmail] = React.useState("");
  const verify = useMutation({
    // Without a password the request carries none at all: that is the
    // "confirm and remove any stored password" path.
    mutationFn: (withPassword: string | undefined) =>
      withPassword === undefined ? api.verifyEmail(token ?? "") : api.verifyEmail(token ?? "", withPassword),
  });
  const resend = useMutation({ mutationFn: () => api.resendConfirmation(email.trim()) });

  // A wrong password leaves the link working: stay on the form and say so.
  const mismatch = verify.error instanceof ApiError && verify.error.code === "password_mismatch";
  const state: "asking" | "confirmed" | "failed" = !token
    ? "failed"
    : verify.isSuccess
      ? "confirmed"
      : verify.isError && !mismatch
        ? "failed"
        : "asking";

  return (
    <div className="grid min-h-dvh place-items-center bg-background px-4 text-foreground">
      <div className="w-full max-w-sm rounded-lg border bg-card p-6" data-testid="verify-page">
        <div className="mb-4 flex items-center gap-2 font-serif text-lg font-semibold">
          <Mark className="text-primary" />
          Vyasa
        </div>

        {state === "asking" ? (
          <>
            <h1 className="text-lg font-semibold">Confirm your email</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Enter the password you chose when you signed up to finish.
            </p>
            <p className="mt-2 text-sm text-muted-foreground" data-testid="verify-not-me">
              If you didn't sign up, you can simply close this page.
            </p>
            <form
              onSubmit={(e) => { e.preventDefault(); verify.mutate(password); }}
              className="mt-4 space-y-4"
            >
              <Field
                label="Password"
                htmlFor="verify-password"
                hint="The password you chose when you signed up."
              >
                <Input
                  id="verify-password"
                  type="password"
                  autoComplete="current-password"
                  autoFocus
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  aria-invalid={mismatch || undefined}
                />
              </Field>
              {mismatch ? (
                <p className="text-sm text-destructive" role="alert" data-testid="verify-mismatch">
                  That isn't the password this account was created with. Try again, or use the option below.
                </p>
              ) : null}
              <Button type="submit" className="w-full" disabled={verify.isPending || password === ""}>
                {verify.isPending ? "Confirming…" : "Confirm"}
              </Button>
            </form>
            <div className="mt-4 border-t pt-4">
              <Button
                type="button"
                variant="outline"
                className="h-auto w-full whitespace-normal py-2 text-left text-sm"
                disabled={verify.isPending}
                onClick={() => verify.mutate(undefined)}
              >
                I don't know the password — email me a link to set one
              </Button>
              <p className="mt-2 text-xs text-muted-foreground">
                Any password typed when this account was created is removed, so only the person reading this mailbox can choose one.
              </p>
            </div>
          </>
        ) : state === "confirmed" ? (
          <>
            <h1 className="text-lg font-semibold">Your email is confirmed</h1>
            <p className="mt-2 text-sm text-muted-foreground" data-testid="verify-done">
              If you confirmed with your password, you can sign in now. Otherwise, check your inbox: we've sent a link to set a password.
            </p>
            <Button className="mt-4 w-full" onClick={() => window.location.assign("/admin/login")}>
              Sign in
            </Button>
          </>
        ) : (
          <>
            <h1 className="text-lg font-semibold">This confirmation link is invalid or has expired</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Open the link from the email again, or send a new one below.
            </p>
            {resend.isSuccess ? (
              <p className="mt-4 text-sm text-muted-foreground" data-testid="resend-sent">
                If that address needs confirming, we've sent a new link. It works for 24 hours.
              </p>
            ) : (
              <form
                onSubmit={(e) => { e.preventDefault(); resend.mutate(); }}
                className="mt-4 space-y-4"
              >
                <Field label="Email address" htmlFor="verify-resend-email">
                  <Input
                    id="verify-resend-email"
                    type="email"
                    required
                    autoFocus
                    autoCapitalize="none"
                    spellCheck={false}
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                  />
                </Field>
                <Button type="submit" className="w-full" disabled={resend.isPending || email.trim() === ""}>
                  {resend.isPending ? "Sending…" : "Send a new link"}
                </Button>
              </form>
            )}
            <a href="/admin/login" className="mt-4 block text-center text-sm text-muted-foreground underline-offset-2 hover:underline">
              Back to sign in
            </a>
          </>
        )}
      </div>
    </div>
  );
}
