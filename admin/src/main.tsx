import { MutationCache, QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRouter, RouterProvider } from "@tanstack/react-router";
import React from "react";
import ReactDOM from "react-dom/client";
import { routeTree } from "./routeTree.gen";
import { api, type UserResponse } from "@/api/client";
import { ThemeProvider } from "@/lib/theme";
import { I18nProvider } from "@/lib/i18n";
import { Mark } from "@/components/ui/logo";
import { clearSessionExpired, safeRedirect, sessionErrorHandler } from "@/lib/session";
import "@/styles/globals.css";

// A 401 anywhere after sign-in means the session is gone: say so once and
// send the author to sign in, with the way back. See lib/session.ts.
const onApiError = (error: unknown) => handleSessionError(error);
const queryClient: QueryClient = new QueryClient({
  queryCache: new QueryCache({
    onError: onApiError,
    onSuccess: (data, query) => {
      // Signed back in (here or in another tab): the session is live again.
      if (query.queryKey[0] === "me" && data != null) clearSessionExpired();
    },
  }),
  mutationCache: new MutationCache({ onError: onApiError }),
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
});

const router = createRouter({
  routeTree,
  context: { queryClient },
  defaultPreload: "intent",
  basepath: "/admin",
});

const handleSessionError = sessionErrorHandler(queryClient, () => {
  const back = safeRedirect(router.state.location.href);
  // Route blockers still run: an editor with unsaved work asks first, and
  // staying keeps the work under the "session expired" banner.
  void router.navigate({ to: "/login", search: back === null ? {} : { redirect: back } });
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

function App() {
  const [ready, setReady] = React.useState(false);

  React.useEffect(() => {
    let cancelled = false;
    // Seed the `me` cache before mounting so the route guards never see an
    // empty cache while the session is still being resolved.
    api
      .me()
      .then((user: UserResponse) => {
        if (!cancelled) queryClient.setQueryData(["me"], user);
      })
      .catch(() => {
        // unauthenticated — leave the cache empty
      })
      .finally(() => {
        if (!cancelled) setReady(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!ready) {
    return (
      <div className="flex min-h-[100dvh] flex-col items-center justify-center gap-3">
        <Mark className="h-9 w-9 animate-pulse text-primary" />
        <span className="text-sm text-muted-foreground">Loading Vyasa…</span>
      </div>
    );
  }

  return (
    <RouterProvider router={router} context={{ queryClient }} />
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>
      <I18nProvider>
        <App />
      </I18nProvider>
      </ThemeProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
