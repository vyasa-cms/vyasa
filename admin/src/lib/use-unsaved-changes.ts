import * as React from "react";
import { useBlocker } from "@tanstack/react-router";
import { useConfirm } from "@/components/ui/dialog";

/**
 * Protect internal navigation and browser close/reload using the same dirty state.
 *
 * Returns `canLeave` (ask, without navigating) and `leave(go)`: ask once,
 * then run `go` with the blocker stood down for that navigation, so an
 * explicit "back" button doesn't ask "Leave without saving?" twice.
 */
export function useUnsavedChanges(isDirty: () => boolean, flush?: () => Promise<void>) {
  const confirm = useConfirm();
  const bypass = React.useRef(false);
  const canLeave = async () => {
    if (!isDirty()) return true;
    if (flush) {
      try { await flush(); if (!isDirty()) return true; } catch { /* Keep edits and offer a choice. */ }
    }
    return confirm({ title: "Leave without saving?", description: "Your unsaved changes will be lost.", confirmLabel: "Leave", destructive: true });
  };
  useBlocker({
    shouldBlockFn: async () => {
      if (bypass.current) {
        // Already confirmed by `leave`; consume it for this navigation only.
        bypass.current = false;
        return false;
      }
      return !(await canLeave());
    },
    enableBeforeUnload: isDirty,
  });
  const leave = async (go: () => void) => {
    if (!(await canLeave())) return false;
    bypass.current = true;
    go();
    return true;
  };
  return Object.assign(canLeave, { leave });
}
