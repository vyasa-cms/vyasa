import { createFileRoute, redirect } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { LoginForm } from "@/components/auth";
import { ThemeToggle } from "@/components/ui/theme-toggle";
import { Mark } from "@/components/ui/logo";
import { api, type UserResponse } from "@/api/client";
import { isSessionExpired } from "@/lib/session";
import { useI18n } from "@/lib/i18n";

export const Route = createFileRoute("/login")({
  // Where to go back to after signing in (set when a session expired or a
  // guarded page was opened signed out).
  validateSearch: (search: Record<string, unknown>): { redirect?: string } =>
    typeof search["redirect"] === "string" ? { redirect: search["redirect"] } : {},
  beforeLoad: async ({ context }) => {
    // An expired session still has a (stale) `me` cached; it must be able
    // to reach the form.
    if (context.queryClient.getQueryData<UserResponse>(["me"]) && !isSessionExpired()) {
      throw redirect({ to: "/" });
    }
    // A site with no administrator has nothing to sign in to yet.
    const status = await Promise.resolve()
      .then(() => api.setupStatus())
      .catch(() => null);
    if (status?.needs_admin) {
      throw redirect({ to: "/setup" });
    }
  },
  component: LoginPage,
});

/**
 * Split screen from `lg` up: form on the left, an identity panel on the right.
 * Below `lg` the panel drops away entirely and the form fills the viewport —
 * a decorative half-screen is dead weight on a phone.
 */
function LoginPage() {
  const { t } = useI18n();
  // Drawing the line between "sign in" and "you can also sign up here" is
  // the site owner's call (`registration_enabled`); the link only exists
  // once they've turned it on and the server can actually send the
  // confirmation (`available`), so nobody fills in a form that would 503.
  const registration = useQuery({
    queryKey: ["registration-info"],
    queryFn: () => api.registrationInfo(),
    retry: false,
    staleTime: 60_000,
  });
  return (
    <div className="grid min-h-dvh lg:grid-cols-2">
      <div className="flex flex-col px-5 py-8 sm:px-10">
        <div className="flex items-center gap-2">
          <Mark className="h-7 w-7 text-primary" />
          <span className="font-serif text-base font-semibold tracking-tight">
            Vyasa
          </span>
          <ThemeToggle className="ml-auto" />
        </div>

        <div className="flex flex-1 items-center justify-center py-10">
          <LoginForm className="w-full max-w-sm" />
        </div>

        <p className="text-center text-xs text-muted-foreground">
          {t("login.trouble")}{" "}
          <a href="/admin/forgot" className="text-primary underline-offset-2 hover:underline">
            {t("login.reset")}
          </a>
          .
        </p>
        {registration.data?.available ? (
          <p className="mt-1 text-center text-xs text-muted-foreground">
            {t("login.new_here")}{" "}
            <a href="/admin/register" className="text-primary underline-offset-2 hover:underline">
              {t("login.create_account")}
            </a>
            .
          </p>
        ) : null}
      </div>

      <div className="relative hidden overflow-hidden border-l bg-muted/40 lg:block">
        <div className="flex h-full flex-col justify-center gap-8 px-12">
          <blockquote className="max-w-md space-y-4">
            <p className="text-2xl font-semibold leading-snug tracking-tight">
              Write, publish, and shape how it all looks — without leaving the
              browser.
            </p>
            <footer className="text-sm text-muted-foreground">
              A content system built in Rust.
            </footer>
          </blockquote>

          <div className="flex flex-wrap gap-2">
            {[
              "Block editor",
              "AI theme studio",
              "Sandboxed plugins",
              "Full-text search",
            ].map((feature) => (
              <span
                key={feature}
                className="rounded-full border bg-card px-3 py-1 text-xs text-muted-foreground"
              >
                {feature}
              </span>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
