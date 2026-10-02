import * as React from "react";
import type { QueryClient } from "@tanstack/react-query";

import { ApiError } from "@/api/client";

/**
 * Session expiry, noticed once for the whole app.
 *
 * Any query or mutation that comes back 401 while someone is signed in
 * means the cookie is gone (expired, revoked, signed out in another tab).
 * The app marks the session expired, invalidates `me`, and sends the
 * author to sign in with a way back. Navigation goes through the normal
 * route blockers, so an editor with unsaved work asks before it is left;
 * choosing to stay keeps the work on screen under a "session expired"
 * banner rather than silently dropping it.
 */

let expired = false;
const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

export function isUnauthorized(error: unknown): boolean {
  return error instanceof ApiError && error.status === 401;
}

export function isSessionExpired(): boolean {
  return expired;
}

export function markSessionExpired(): boolean {
  if (expired) return false;
  expired = true;
  emit();
  return true;
}

/** Called when a sign-in succeeds. */
export function clearSessionExpired(): void {
  if (!expired) return;
  expired = false;
  emit();
}

export function useSessionExpired(): boolean {
  return React.useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => expired,
    () => expired,
  );
}

/**
 * A redirect target that stays inside the admin: an absolute path, not a
 * protocol-relative URL, and not the login page itself.
 */
export function safeRedirect(target: unknown): string | null {
  if (typeof target !== "string") return null;
  if (!target.startsWith("/") || target.startsWith("//") || target.startsWith("/\\")) return null;
  if (target === "/login" || target.startsWith("/login?") || target.startsWith("/login/")) return null;
  return target;
}

/**
 * The `onError` for the QueryCache and MutationCache.
 *
 * `goToLogin` is handed the in-app location to come back to; it is only
 * called for the first 401 after a signed-in state, so a burst of failing
 * requests produces one redirect.
 */
export function sessionErrorHandler(
  queryClient: QueryClient,
  goToLogin: () => void,
): (error: unknown) => void {
  return (error) => {
    if (!isUnauthorized(error)) return;
    // Not signed in to begin with (the login page, first load): nothing
    // expired.
    if (queryClient.getQueryData(["me"]) == null) return;
    if (!markSessionExpired()) return;
    void queryClient.invalidateQueries({ queryKey: ["me"] });
    goToLogin();
  };
}
