import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation } from "@tanstack/react-query";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { Mark } from "@/components/ui/logo";

export const Route = createFileRoute("/forgot")({ component: ForgotPage });

/** Always the same answer, whatever the address: no account enumeration. */
function ForgotPage() {
  const [email, setEmail] = React.useState("");
  const send = useMutation({ mutationFn: () => api.forgotPassword(email.trim()) });
  return (
    <div className="grid min-h-dvh place-items-center bg-background px-4 text-foreground">
      <div className="w-full max-w-sm rounded-lg border bg-card p-6" data-testid="forgot-page">
        <div className="mb-4 flex items-center gap-2 font-serif text-lg font-semibold"><Mark className="text-primary" />Vyasa</div>
        {send.isSuccess ? (
          <>
            <h1 className="text-lg font-semibold">Check your inbox</h1>
            <p className="mt-2 text-sm text-muted-foreground">If an account exists for {email.trim()}, a link to set a new password is on its way. It works for one hour.</p>
            <a href="/admin/login" className="mt-4 inline-block text-sm text-primary underline-offset-2 hover:underline">Back to sign in</a>
          </>
        ) : (
          <form onSubmit={(e) => { e.preventDefault(); send.mutate(); }} className="space-y-4">
            <h1 className="text-lg font-semibold">Reset your password</h1>
            <Field label="Email address" htmlFor="forgot-email">
              <Input id="forgot-email" type="email" required autoFocus value={email} onChange={(e) => setEmail(e.target.value)} />
            </Field>
            <Button type="submit" className="w-full" disabled={send.isPending || email.trim() === ""}>{send.isPending ? "Sending…" : "Send reset link"}</Button>
            <a href="/admin/login" className="block text-center text-sm text-muted-foreground underline-offset-2 hover:underline">Back to sign in</a>
          </form>
        )}
      </div>
    </div>
  );
}
