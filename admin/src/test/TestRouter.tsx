import * as React from "react";
import { createRootRoute, createRouter, createMemoryHistory, RouterProvider } from "@tanstack/react-router";

/** Real router context so mounted page tests exercise navigation blockers. */
export function TestRouter({ children }: { children: React.ReactNode }) {
  const [router] = React.useState(() => createRouter({
    routeTree: createRootRoute({ component: () => <>{children}</> }),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  }));
  return <RouterProvider router={router} />;
}
