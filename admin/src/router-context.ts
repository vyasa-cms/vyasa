import type { QueryClient } from "@tanstack/react-query";

/** Values injected into every route's TanStack Router context. */
export interface RouterContextValue {
  queryClient: QueryClient;
}
