import { useCapabilities, canVisit, profileOnly } from "@/lib/capabilities";
import { ErrorNote } from "@/components/ui/primitives";
import * as React from "react";
import {
  createFileRoute,
  redirect,
  Outlet,
  Link,
  useNavigate,
  useLocation,
  useMatches,
} from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import {
  BarChart3,
  Boxes,
  Plus,
  ExternalLink,
  FileText,
  Files,
  Image,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Menu as MenuIcon,
  MessageSquare,
  Palette,
  PanelLeftClose,
  PanelLeftOpen,
  Plug,
  Settings,
  Sparkles,
  Stethoscope,
  Tags,
  Users,
  Webhook,
  X,
  Bookmark,
  ClipboardList,
  ShieldCheck,
} from "lucide-react";
import { useMe, useLogout } from "@/components/auth";
import { useI18n } from "@/lib/i18n";
import { Button } from "@/components/ui/button";
import { ConfirmProvider } from "@/components/ui/dialog";
import { Toaster } from "@/components/ui/toast";
import { ThemeToggle, ThemeCycleButton } from "@/components/ui/theme-toggle";
import { Mark } from "@/components/ui/logo";
import { api, type UserResponse } from "@/api/client";
import { cn } from "@/lib/utils";
import { safeRedirect, useSessionExpired } from "@/lib/session";

/**
 * Auth guard: reads the `me` query cache (seeded before the router mounts,
 * so there is no flash of unauthenticated content).
 */
export const Route = createFileRoute("/_auth")({
  beforeLoad: ({ context, location }) => {
    const me = context.queryClient.getQueryData<UserResponse>(["me"]);
    if (!me) {
      const back = safeRedirect(location.href);
      throw redirect({ to: "/login", search: back === null ? {} : { redirect: back } });
    }
  },
  component: AuthLayout,
});

interface NavItem {
  to: string;
  label: string;
  icon: typeof LayoutDashboard;
  /** A content type's own entry: its label is the type's name, not a dictionary key. */
  type?: { slug: string; singular: string };
}

/** Grouped so the sidebar reads as three jobs, not twelve links. */
const NAV_GROUPS: { heading: string; items: NavItem[] }[] = [
  {
    heading: "nav.content",
    items: [
      { to: "/", label: "nav.dashboard", icon: LayoutDashboard },
      { to: "/posts", label: "nav.posts", icon: FileText },
      { to: "/pages", label: "nav.pages", icon: Files },
      { to: "/media", label: "nav.media", icon: Image },
      { to: "/comments", label: "nav.comments", icon: MessageSquare },
      { to: "/forms", label: "nav.forms", icon: ClipboardList },
      { to: "/audience", label: "nav.audience", icon: BarChart3 },
      { to: "/taxonomy", label: "nav.categories", icon: Tags },
    ],
  },
  {
    heading: "nav.design",
    items: [
      { to: "/appearance", label: "nav.appearance", icon: Palette },
      { to: "/menus", label: "nav.menus", icon: MenuIcon },
      { to: "/patterns", label: "nav.patterns", icon: Bookmark },
    ],
  },
  {
    heading: "nav.site",
    items: [
      { to: "/plugins", label: "nav.plugins", icon: Plug },
      { to: "/models", label: "nav.models", icon: Sparkles },
      { to: "/content-types", label: "nav.content_types", icon: Boxes },
      { to: "/users", label: "nav.users", icon: Users },
      { to: "/roles", label: "nav.roles", icon: KeyRound },
      { to: "/webhooks", label: "nav.webhooks", icon: Webhook },
      { to: "/health", label: "nav.health", icon: Stethoscope },
      { to: "/privacy", label: "nav.privacy", icon: ShieldCheck },
      { to: "/settings", label: "nav.settings", icon: Settings },
    ],
  },
];

/**
 * Routes may declare themselves workbenches.
 *
 * Augmenting the router's own option rather than keeping a list of paths
 * in the layout: the page that needs the width is the page that says so,
 * and a route that moves takes the declaration with it.
 */
declare module "@tanstack/react-router" {
  interface StaticDataRouteOption {
    /** Use the whole window rather than the reading-width column. */
    wide?: boolean;
  }
}

/** Injected at build time by vite.config.ts. */
declare const __VYASA_BUILD__: string;
const BUILD_ID: string = typeof __VYASA_BUILD__ === "string" ? __VYASA_BUILD__ : "dev";

/**
 * Whether the page is running a bundle the server has moved on from.
 *
 * Split out from the reload so the decision is testable: `null` on either
 * side means "cannot tell", which must never trigger a reload.
 */
export function staleShell(
  loaded: string | null,
  current: string | null | undefined,
): boolean {
  if (loaded === null || current === null || current === undefined) return false;
  return loaded !== current;
}

/** The bundle this page actually loaded, from its own script tag. */
function loadedBundle(): string | null {
  if (typeof document === "undefined") return null;
  const src = [...document.querySelectorAll<HTMLScriptElement>("script[src]")]
    .map((s) => s.getAttribute("src") ?? "")
    .find((s) => s.includes("/assets/index-"));
  return src === undefined ? null : src.split("/").pop() ?? null;
}

/**
 * Reloads once when the shell is older than the server.
 *
 * `index.html` names which bundle is current, so a browser holding a stale
 * copy runs an old app while the server version and migration number both
 * read correctly — the deploy looks landed and is not. Comparing the two
 * is the only reliable signal, and one guarded reload fixes it without
 * anyone being told to clear a cache.
 */
function useReloadIfStale(current: string | null | undefined): void {
  React.useEffect(() => {
    if (current === null || current === undefined) return;
    if (!staleShell(loadedBundle(), current)) return;
    // Guarded: if a reload somehow does not fix it, do not loop forever.
    const key = `vyasa-reloaded-for-${current}`;
    if (sessionStorage.getItem(key) !== null) return;
    sessionStorage.setItem(key, "1");
    window.location.reload();
  }, [current]);
}

/**
 * The running build, in the sidebar footer.
 *
 * Worth the pixels because the commonest deploy question is "did my change
 * actually land". The migration number is shown alongside: a binary and a
 * database at different versions is the failure that looks like corruption.
 */
function BuildVersion() {
  const build = useQuery({
    queryKey: ["version"],
    queryFn: () => api.version(),
    staleTime: Infinity,
  });
  useReloadIfStale(build.data?.ui_bundle);
  if (build.data === undefined) return null;
  return (
    <p
      className="px-2.5 text-center text-[11px] text-muted-foreground"
      data-testid="build-stamp"
    >
      v{build.data.version}
      {build.data.migration_version === null
        ? null
        : ` · db ${build.data.migration_version}`}
      {/* The bundle, not the server. A browser holding a stale shell shows
          the right server version and the wrong app; this is the only
          thing on screen that distinguishes them. */}
      {` · ui ${BUILD_ID}`}
    </p>
  );
}

const COLLAPSED_KEY = "vyasa-nav-collapsed";

/**
 * Desktop sidebar width, remembered per browser. Below the desktop
 * breakpoint the sidebar is always a drawer, so this only applies there.
 */
function useCollapsed(): [boolean, () => void] {
  const [collapsed, setCollapsed] = React.useState(() => {
    try {
      return localStorage.getItem(COLLAPSED_KEY) === "1";
    } catch {
      return false;
    }
  });
  const toggle = React.useCallback(() => {
    setCollapsed((v) => {
      try {
        localStorage.setItem(COLLAPSED_KEY, v ? "0" : "1");
      } catch {
        // Storage unavailable — the choice just won't outlive the tab.
      }
      return !v;
    });
  }, []);
  return [collapsed, toggle];
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).slice(0, 2);
  const letters = parts.map((p) => p.charAt(0).toUpperCase()).join("");
  return letters === "" ? "?" : letters;
}

function AuthLayout() {
  const caps = useCapabilities();
  const location = useLocation();
  const me = useMe();
  const logout = useLogout();
  const navigate = useNavigate();
  const user = me.data;
  const { t } = useI18n();
  // Phones and tablets get a hamburger drawer; desktop gets a sidebar that
  // can collapse to an icon rail. The same markup serves both.
  const [mobileOpen, setMobileOpen] = React.useState(false);
  const [collapsed, toggleCollapsed] = useCollapsed();
  const sessionExpired = useSessionExpired();
  // After signing in from another tab, coming back here notices it.
  const refetchMe = me.refetch;
  React.useEffect(() => {
    if (!sessionExpired) return;
    const onFocus = () => void refetchMe();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [sessionExpired, refetchMe]);

  // Without `view_admin` the account has its profile and sign-out, and
  // nothing else: no navigation, and any other address leads back to the
  // profile. Decided only once the grants are known, never while loading.
  const restricted = caps.data !== undefined && profileOnly(caps.data);
  // The matched route, not the address: the address changes a moment
  // before the page under `<Outlet />` does, and that moment must not
  // render (or fetch for) the page being left.
  const onProfile = useMatches({ select: (matches) => matches.some((m) => m.routeId.startsWith("/_auth/profile")) });
  React.useEffect(() => {
    if (restricted && !onProfile) void navigate({ to: "/profile", replace: true });
  }, [restricted, onProfile, navigate]);

  // The one count worth surfacing in navigation: work waiting on a human.
  const pending = useQuery({
    queryKey: ["comments", "count", "pending"],
    queryFn: () => api.listCommentsPage({ status: "pending", limit: 1 }),
    staleTime: 60_000,
    enabled: caps.can("moderate_comments"),
  });
  const pendingCount = pending.data?.total ?? 0;

  // Plugins' and administrators' content types each get an entry after
  // Pages: their entries, and a quick way to start one.
  const contentTypes = useQuery({
    queryKey: ["content-types"],
    queryFn: () => api.listContentTypes(),
    staleTime: 60_000,
    enabled: caps.can("edit_posts"),
  });
  const navGroups = React.useMemo(() => {
    const custom: NavItem[] = (contentTypes.data ?? [])
      .filter((t) => t.owner !== "builtin")
      .map((t) => ({ to: `/entries/${t.slug}`, label: t.plural, icon: Boxes, type: { slug: t.slug, singular: t.singular } }));
    if (custom.length === 0) return NAV_GROUPS;
    return NAV_GROUPS.map((group) => {
      const at = group.items.findIndex((item) => item.to === "/pages");
      if (at < 0) return group;
      return { ...group, items: [...group.items.slice(0, at + 1), ...custom, ...group.items.slice(at + 1)] };
    });
  }, [contentTypes.data]);

  // Close the drawer on Escape, and lock the page behind it.
  React.useEffect(() => {
    if (!mobileOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMobileOpen(false);
    };
    const { overflow } = document.body.style;
    document.body.style.overflow = "hidden";
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.body.style.overflow = overflow;
    };
  }, [mobileOpen]);

  const nav = (
    <nav
      className={cn(
        "flex-1 space-y-4 overflow-y-auto px-2 py-3",
        collapsed && "lg:space-y-2 lg:px-1.5",
      )}
      aria-label="Admin"
    >
      {navGroups.map(group => ({ ...group, items: group.items.filter(item => canVisit(item.to, caps.data ?? [])) })).filter(group => group.items.length > 0).map((group) => (
        <div key={t(group.heading)}>
          <p
            className={cn(
              "px-2 pb-1 text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground",
              collapsed && "lg:sr-only",
            )}
          >
            {t(group.heading)}
          </p>
          <div className={cn("space-y-0.5", collapsed && "lg:border-t lg:pt-1.5")}>
            {group.items.map((item) => {
              const Icon = item.icon;
              const badge =
                item.to === "/comments" && pendingCount > 0 ? pendingCount : null;
              const label = item.type === undefined ? t(item.label) : item.label;
              const link = (
                <Link
                  key={item.to}
                  to={item.to}
                  aria-label={badge !== null ? `${label} (${badge} pending)` : label}
                  title={collapsed ? label : undefined}
                  onClick={() => setMobileOpen(false)}
                  className={cn(
                    "touch-target relative flex items-center gap-2.5 rounded-md px-2.5 py-2 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground lg:py-1.5",
                    collapsed && "lg:h-10 lg:justify-center lg:px-0",
                  )}
                  activeProps={{
                    // The studio's active-state language: a tint, not a
                    // filled block — the accent marks where you are
                    // without shouting over the content.
                    className:
                      "bg-primary/15 text-primary hover:bg-primary/15 hover:text-primary font-medium",
                  }}
                  activeOptions={{ exact: item.to === "/" }}
                >
                  <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
                  <span className={cn("truncate", collapsed && "lg:hidden")}>{label}</span>
                  {badge !== null ? (
                    <span
                      aria-hidden="true"
                      className={cn(
                        "ml-auto rounded-full bg-warning-subtle px-1.5 py-0.5 text-[10px] font-bold text-warning",
                        collapsed &&
                          "lg:absolute lg:right-1.5 lg:top-1 lg:ml-0 lg:h-2 lg:w-2 lg:bg-warning lg:p-0 lg:text-[0px]",
                      )}
                    >
                      {badge}
                    </span>
                  ) : null}
                </Link>
              );
              if (item.type === undefined) return link;
              return (
                <div key={item.to} className="group relative">
                  {link}
                  <Link
                    to="/posts/$postId"
                    params={{ postId: "new" }}
                    search={{ type: item.type.slug }}
                    aria-label={`New ${item.type.singular}`}
                    title={`New ${item.type.singular}`}
                    onClick={() => setMobileOpen(false)}
                    className={cn(
                      "absolute right-1 top-1/2 inline-flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground",
                      collapsed && "lg:hidden",
                    )}
                  >
                    <Plus className="h-3.5 w-3.5" aria-hidden="true" />
                  </Link>
                </div>
              );
            })}
          </div>
        </div>
      ))}
    </nav>
  );

  return (
    <ConfirmProvider>
      <div className="flex min-h-[100dvh] bg-background">
        {/* Mobile drawer backdrop */}
        {mobileOpen && !restricted ? (
          <button
            type="button"
            aria-label="Close navigation"
            onClick={() => setMobileOpen(false)}
            className="fixed inset-0 z-30 animate-fade-in bg-black/50 backdrop-blur-[1px] lg:hidden"
          />
        ) : null}

        {restricted ? null : (
        <aside
          id="admin-sidebar"
          className={cn(
            "fixed inset-y-0 left-0 z-40 flex w-[17rem] max-w-[82vw] flex-col border-r bg-card transition-[transform,width] duration-200 ease-out",
            // Desktop: sticky, so the navigation stays put while a long post
            // scrolls and never scrolls out of reach. Collapsible to a rail.
            "lg:sticky lg:top-0 lg:z-auto lg:h-dvh lg:max-w-none lg:translate-x-0 lg:self-start",
            collapsed ? "lg:w-[4.25rem]" : "lg:w-56",
            mobileOpen ? "translate-x-0 shadow-2xl" : "-translate-x-full lg:shadow-none",
          )}
        >
          <div
            className={cn(
              "flex h-14 shrink-0 items-center gap-2 border-b px-3",
              collapsed && "lg:flex-col lg:h-auto lg:gap-1 lg:px-0 lg:py-2",
            )}
          >
            <Mark className="h-6 w-6 shrink-0 text-primary" />
            <span
              className={cn(
                "truncate font-serif text-[15px] font-semibold tracking-tight",
                collapsed && "lg:sr-only",
              )}
            >
              Vyasa
            </span>
            <button
              type="button"
              onClick={() => setMobileOpen(false)}
              className="ml-auto inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent lg:hidden"
              aria-label="Close menu"
            >
              <X className="h-4 w-4" aria-hidden="true" />
            </button>
            <button
              type="button"
              onClick={toggleCollapsed}
              aria-expanded={!collapsed}
              aria-controls="admin-sidebar"
              aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
              title={collapsed ? "Expand navigation" : "Collapse navigation"}
              className={cn(
                "hidden h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent lg:inline-flex",
                !collapsed && "ml-auto",
              )}
            >
              {collapsed ? (
                <PanelLeftOpen className="h-4 w-4" aria-hidden="true" />
              ) : (
                <PanelLeftClose className="h-4 w-4" aria-hidden="true" />
              )}
            </button>
          </div>

          {nav}

          <div className={cn("shrink-0 space-y-2 border-t p-2.5", collapsed && "lg:p-1.5")}>
            <a
              href="/"
              target="_blank"
              rel="noreferrer"
              title={collapsed ? t("topbar.view_site") : undefined}
              className={cn(
                "flex items-center gap-2 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                collapsed && "lg:h-10 lg:justify-center lg:px-0",
              )}
            >
              <ExternalLink className="h-4 w-4" aria-hidden="true" />
              <span className={cn(collapsed && "lg:hidden")}>{t("topbar.view_site")}</span>
            </a>
            <ThemeToggle className={cn("w-full justify-center", collapsed && "lg:hidden")} />
            <ThemeCycleButton className={cn("hidden", collapsed && "lg:mx-auto lg:flex")} />
            <div className={cn(collapsed && "lg:hidden")}>
              <BuildVersion />
            </div>
          </div>
        </aside>
        )}

        <div className="flex min-w-0 flex-1 flex-col">
          <header className="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-2 border-b bg-card/85 px-3 backdrop-blur supports-[backdrop-filter]:bg-card/70 md:px-5">
            {restricted ? null : (
              <button
                type="button"
                onClick={() => setMobileOpen(true)}
                className="inline-flex h-9 w-9 items-center justify-center rounded-md border bg-background text-muted-foreground lg:hidden"
                aria-label="Open navigation"
                aria-expanded={mobileOpen}
                aria-controls="admin-sidebar"
              >
                <MenuIcon className="h-4 w-4" aria-hidden="true" />
              </button>
            )}
            <span className={cn("truncate text-sm font-semibold", !restricted && "lg:hidden")}>
              Vyasa
            </span>

            <div className="ml-auto flex items-center gap-2">
              <ThemeCycleButton className="lg:hidden" />
              {user ? (
                <>
                  <span className="hidden items-center gap-2 sm:flex">
                    <span
                      className="flex h-7 w-7 items-center justify-center rounded-full bg-primary-subtle text-[10px] font-bold text-primary"
                      aria-hidden="true"
                    >
                      {user.avatar_url ? <img src={user.avatar_url} alt="" className="h-full w-full rounded-full object-cover" /> : initials(user.display_name)}
                    </span>
                    <span className="flex flex-col leading-tight">
                      <a
                        href="/admin/profile"
                        data-testid="topbar-user"
                        className="max-w-[140px] truncate text-xs font-medium underline-offset-2 hover:underline"
                        title={t("topbar.profile")}
                      >
                        {user.display_name}
                      </a>
                      {/* The role's name: under a custom role the built-in
                          `role` is always "subscriber", which is not what
                          this person is. */}
                      <span data-testid="topbar-role" className="max-w-[140px] truncate text-[10px] uppercase tracking-wide text-muted-foreground">
                        {user.role_name}
                      </span>
                    </span>
                  </span>
                  <Button
                    variant="outline"
                    size="sm"
                    data-testid="logout-button"
                    onClick={() =>
                      logout.mutate(undefined, {
                        onSuccess: () => {
                          void navigate({ to: "/login" });
                        },
                      })
                    }
                    className="h-8"
                  >
                    <LogOut className="h-3.5 w-3.5" aria-hidden="true" />
                    <span className="hidden sm:inline">{t("topbar.log_out")}</span>
                  </Button>
                </>
              ) : null}
            </div>
          </header>

          <main className="flex-1 p-3 sm:p-4 md:p-6">
            {/* No width cap: on an ultrawide monitor a 1200px column left
                most of the screen empty. Pages that want a measure set one
                on their own forms; lists, tables and workbenches use the
                room. */}
            <div className="mx-auto w-full">
              {sessionExpired ? (
                <div
                  role="alert"
                  data-testid="session-expired"
                  className="mb-4 flex flex-wrap items-center gap-3 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm"
                >
                  <span className="min-w-0 flex-1">
                    <span className="font-medium text-destructive">Your session has expired.</span>{" "}
                    Nothing more can be saved until you sign in again. Unsaved work on this page
                    stays here — sign in in a new tab, then come back and save.
                  </span>
                  <a
                    href="/admin/login"
                    target="_blank"
                    rel="noopener"
                    className="inline-flex h-8 items-center rounded-md border bg-background px-3 text-sm hover:bg-accent"
                  >
                    Sign in in a new tab
                  </a>
                  <Button
                    size="sm"
                    onClick={() => {
                      const back = safeRedirect(location.href);
                      void navigate({ to: "/login", search: back === null ? {} : { redirect: back } });
                    }}
                  >
                    Sign in here
                  </Button>
                </div>
              ) : null}
              {/* A failed background refetch keeps the cached grants: only a
                  first load with nothing to show replaces the page (and
                  never unmounts an open editor over a blip). */}
              {restricted ? (
                <p
                  role="status"
                  data-testid="profile-only-note"
                  className="mb-4 rounded-md border bg-muted px-3 py-2 text-sm text-muted-foreground"
                >
                  You can sign in and manage your own profile, but can&rsquo;t use the admin screens.
                  Ask an administrator if you need more.
                </p>
              ) : null}
              {caps.isPending ? <p role="status">Loading permissions…</p> : caps.data === undefined && caps.isError ? <ErrorNote title="Couldn't load permissions" error={caps.error} onRetry={() => void caps.refetch()} /> : restricted ? (onProfile ? <Outlet /> : null) : canVisit(location.pathname, caps.data ?? []) ? <Outlet /> : <p role="alert">You do not have permission to access this page.</p>}
            </div>
          </main>
        </div>
      </div>
      <Toaster />
    </ConfirmProvider>
  );
}
