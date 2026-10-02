import {
  type QueryClient,
  type UseQueryResult,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import * as React from "react";
import { Eye, EyeOff } from "lucide-react";
import { api, ApiError } from "@/api/client";
import { useI18n } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { ErrorNote, SkeletonRows } from "@/components/ui/primitives";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { clearSessionExpired, safeRedirect } from "@/lib/session";

/**
 * Auth context: the `me` query result is shared app-wide via TanStack Query
 * cache; this hook is the single accessor.
 */
export function useMe() {
  return useQuery({
    queryKey: ["me"],
    queryFn: () => api.me(),
    retry: false,
    staleTime: 60_000,
  });
}

/**
 * A sign-in starts from an empty cache. The browser may still hold the
 * previous person's data and grants (their session expired, or they closed
 * the tab, without signing out), and whoever signs in next must never be
 * shown either, not even for the moment a refetch takes.
 */
function signedIn(queryClient: QueryClient, user: unknown) {
  clearSessionExpired();
  queryClient.clear();
  queryClient.setQueryData(["me"], user);
}

export function useLogin() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ email, password }: { email: string; password: string }) =>
      api.login(email, password),
    onSuccess: (user) => {
      // A 202 with a challenge is not a user; the form asks for the code.
      if ((user as unknown as { mfa_required?: boolean }).mfa_required) return;
      signedIn(queryClient, user);
    },
  });
}

export function useLogout() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.logout(),
    onSuccess: () => {
      queryClient.setQueryData(["me"], null);
      queryClient.clear();
    },
  });
}

/** Login form with error states. */
export function LoginForm({ className }: { className?: string }) {
  const { t } = useI18n();
  const login = useLogin();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [email, setEmail] = React.useState("");
  const [password, setPassword] = React.useState("");
  const [showPassword, setShowPassword] = React.useState(false);
  const [capsLock, setCapsLock] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  // A wrong-password 401 is indistinguishable from an unconfirmed account
  // with the *right* password — the server answers both the same way so
  // sign-in never reveals which addresses exist. A generic hint (only
  // offered when the site takes registrations at all) is the only way to
  // point someone at "confirm your email" without that leak.
  const [unauthorized, setUnauthorized] = React.useState(false);
  const [resendOpen, setResendOpen] = React.useState(false);
  const [resendEmail, setResendEmail] = React.useState("");
  const registration = useQuery({
    queryKey: ["registration-info"],
    queryFn: () => api.registrationInfo(),
    retry: false,
    staleTime: 60_000,
  });
  const resend = useMutation({ mutationFn: () => api.resendConfirmation(resendEmail.trim()) });
  // The password step passed but a second factor is wanted: the server
  // answered 202 with a short challenge; a code from the app or a
  // recovery code finishes the sign-in.
  const [challenge, setChallenge] = React.useState<string | null>(null);
  const [code, setCode] = React.useState("");
  // Back to where an expired session interrupted, when there was one.
  const search = useSearch({ strict: false }) as { redirect?: unknown };
  const goOn = () => {
    const back = safeRedirect(search.redirect);
    void (back === null ? navigate({ to: "/" }) : navigate({ href: back }));
  };
  const finish = useMutation({
    mutationFn: () => api.loginMfa(challenge ?? "", code.trim()),
    onSuccess: (me) => {
      signedIn(queryClient, me);
      goOn();
    },
    onError: (err) => {
      if (err instanceof ApiError && err.status === 401) setError("That code isn't right. Codes change every 30 seconds; a recovery code also works.");
      else setError(err.message || "Couldn't reach the server.");
    },
  });

  function handleSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    setUnauthorized(false);
    setResendOpen(false);
    resend.reset();
    login.mutate(
      { email, password },
      {
        onSuccess: (me) => {
          if ((me as unknown as { mfa_required?: boolean }).mfa_required) {
            setChallenge((me as unknown as { challenge: string }).challenge);
            return;
          }
          goOn();
        },
        onError: (err) => {
          // Distinguish "we rejected you" from "we never reached the server",
          // because the fix is completely different in each case.
          if (err instanceof ApiError && err.status === 401) {
            setError("That email and password don't match an account.");
            setUnauthorized(true);
          } else if (err instanceof ApiError && err.status === 429) {
            setError("Too many attempts. Wait a minute and try again.");
          } else {
            setError(
              err.message ||
                "Couldn't reach the server. Check your connection and try again.",
            );
          }
        },
      },
    );
  }

  return (
    <Card className={cn("w-full max-w-sm", className)}>
      <CardHeader>
        <CardTitle>{t("login.title")}</CardTitle>
        <CardDescription>{t("login.subtitle")}</CardDescription>
      </CardHeader>
      <CardContent>
        {challenge !== null ? (
          <form onSubmit={(e) => { e.preventDefault(); setError(null); finish.mutate(); }} className="flex flex-col gap-4" data-testid="mfa-step">
            <label className="flex flex-col gap-1.5 text-sm font-medium">
              Code from your authenticator app
              <Input data-testid="mfa-code" autoFocus inputMode="numeric" autoComplete="one-time-code" value={code} onChange={(e) => setCode(e.target.value)} placeholder="123456" className="font-mono" />
            </label>
            <p className="text-xs text-muted-foreground">Lost the phone? A recovery code from when you set this up works here too.</p>
            {error ? <p className="text-sm text-destructive" role="alert">{error}</p> : null}
            <Button type="submit" disabled={finish.isPending || code.trim() === ""}>{finish.isPending ? "Checking…" : "Continue"}</Button>
            <button type="button" className="text-xs text-muted-foreground underline-offset-2 hover:underline" onClick={() => { setChallenge(null); setCode(""); setError(null); }}>Start over</button>
          </form>
        ) : (
        <>
        <form onSubmit={handleSubmit} className="flex flex-col gap-4">
          <label className="flex flex-col gap-1.5 text-sm font-medium">
            {t("login.email")}
            <Input
              data-testid="login-email"
              type="email"
              required
              autoComplete="username"
              autoCapitalize="none"
              spellCheck={false}
              inputMode="email"
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="you@example.com"
              className="font-normal"
            />
          </label>

          <label className="flex flex-col gap-1.5 text-sm font-medium">
            {t("login.password")}
            <span className="relative flex items-center">
              <Input
                data-testid="login-password"
                type={showPassword ? "text" : "password"}
                required
                autoComplete="current-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                onKeyUp={(e) =>
                  setCapsLock(e.getModifierState?.("CapsLock") ?? false)
                }
                placeholder="••••••••"
                className="pr-10 font-normal"
              />
              <button
                type="button"
                onClick={() => setShowPassword((v) => !v)}
                aria-label={showPassword ? "Hide password" : "Show password"}
                className="absolute right-1 inline-flex h-7 w-8 items-center justify-center rounded text-muted-foreground hover:text-foreground"
              >
                {showPassword ? (
                  <EyeOff className="h-4 w-4" aria-hidden="true" />
                ) : (
                  <Eye className="h-4 w-4" aria-hidden="true" />
                )}
              </button>
            </span>
            {capsLock ? (
              <span className="text-xs font-normal text-warning">
                Caps Lock is on.
              </span>
            ) : null}
          </label>

          {error ? (
            <p
              role="alert"
              data-testid="login-error"
              className="rounded-md border border-destructive/40 bg-destructive-subtle px-3 py-2 text-sm text-destructive"
            >
              {error}
            </p>
          ) : null}

          <Button type="submit" disabled={login.isPending}>
            {login.isPending ? "Signing in…" : "Sign in"}
          </Button>
        </form>

        {/*
          Deliberately NOT inside the sign-in `<form>` above (nested, or
          even just sharing one): a `required` field left empty here used
          to block the sign-in submit's native validation, so clicking
          "Sign in" silently did nothing while this panel was open. This
          is its own sibling form, scoped to its own validation and its
          own submit.
        */}
        {unauthorized && registration.data?.available ? (
          <div className="mt-4 space-y-2 rounded-md border bg-muted/40 px-3 py-2 text-xs text-muted-foreground" data-testid="confirm-hint">
            <p>{t("login.confirm_hint")}</p>
            {resend.isSuccess ? (
              <p data-testid="resend-confirmation-sent">{t("login.resend_sent")}</p>
            ) : resendOpen ? (
              <form
                onSubmit={(e) => { e.preventDefault(); if (resendEmail.trim() !== "") resend.mutate(); }}
                className="flex items-center gap-2"
              >
                <Input
                  aria-label={t("login.resend_email_label")}
                  type="email"
                  required
                  autoFocus
                  value={resendEmail}
                  onChange={(e) => setResendEmail(e.target.value)}
                  className="h-7 text-xs font-normal"
                />
                <Button type="submit" size="sm" variant="outline" disabled={resend.isPending || resendEmail.trim() === ""}>
                  {resend.isPending ? "…" : t("login.resend_send")}
                </Button>
                <button type="button" className="text-muted-foreground underline-offset-2 hover:underline" onClick={() => setResendOpen(false)}>
                  {t("login.cancel")}
                </button>
              </form>
            ) : (
              <button
                type="button"
                className="text-primary underline-offset-2 hover:underline"
                onClick={() => { setResendEmail(email.trim()); setResendOpen(true); }}
              >
                {t("login.resend_confirmation")}
              </button>
            )}
          </div>
        ) : null}
        </>
        )}
      </CardContent>
    </Card>
  );
}

/** Renders children for a finished query, or loading/error/empty fallbacks. */
export function QueryBoundary<T>({
  query,
  empty,
  children,
}: {
  query: UseQueryResult<T, Error>;
  empty?: React.ReactNode;
  children: (data: T) => React.ReactNode;
}) {
  if (query.isPending) {
    return (
      <div data-testid="query-loading" className="rounded-lg border bg-card">
        <SkeletonRows />
      </div>
    );
  }
  if (query.isError) {
    return <ErrorNote title="Couldn't load this" error={query.error} />;
  }
  if (empty !== undefined && isEmptyValue(query.data)) {
    return <>{empty}</>;
  }
  return <>{children(query.data)}</>;
}

function isEmptyValue(data: unknown): boolean {
  if (Array.isArray(data)) return data.length === 0;
  return false;
}
