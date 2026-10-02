import { createRootRouteWithContext, Outlet } from "@tanstack/react-router";
import type { RouterContextValue } from "@/router-context";

export const Route = createRootRouteWithContext<RouterContextValue>()({
  component: () => <Outlet />,
});
