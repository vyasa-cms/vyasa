import * as React from "react";

/**
 * Subscribes to a media query.
 *
 * Used to render *one* layout rather than shipping both and hiding one with
 * CSS: duplicated markup makes screen readers announce every row twice and
 * doubles the DOM on long lists.
 *
 * Falls back to `false` where `matchMedia` is unavailable, so server-side and
 * test environments get the desktop layout.
 */
export function useMediaQuery(query: string): boolean {
  const subscribe = React.useCallback(
    (onChange: () => void) => {
      if (
        typeof window === "undefined" ||
        typeof window.matchMedia !== "function"
      ) {
        return () => undefined;
      }
      const mql = window.matchMedia(query);
      mql.addEventListener("change", onChange);
      return () => mql.removeEventListener("change", onChange);
    },
    [query],
  );

  const getSnapshot = React.useCallback(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return false;
    }
    return window.matchMedia(query).matches;
  }, [query]);

  return React.useSyncExternalStore(subscribe, getSnapshot, () => false);
}

/** True below Tailwind's `sm` breakpoint (640px). */
export function useIsMobile(): boolean {
  return useMediaQuery("(max-width: 639px)");
}
