import * as React from "react";
import { api } from "@/api/client";

export interface PostLock {
  /** Someone else holds the lock; the editor should hold back saves. */
  heldByOther: boolean;
  holderName: string | null;
  seenAgoSecs: number | null;
  /** Take the lock from whoever has it. */
  takeOver: () => void;
}

const HEARTBEAT_MS = 30_000;

/**
 * Names who is editing this entry. The lock is taken when the editor
 * opens, refreshed every half minute, and let go on the way out; a
 * second person sees the holder and can take over. This is presence,
 * not live co-editing: two people still write in turns, but they know.
 */
export function usePostLock(postId: string | undefined): PostLock {
  const [state, setState] = React.useState<{ mine: boolean; holder_name: string | null; seen_ago_secs: number | null } | null>(null);
  const forced = React.useRef(false);

  React.useEffect(() => {
    if (postId === undefined) return;
    let cancelled = false;
    const beat = async () => {
      try {
        const s = await api.lockPost(postId, forced.current);
        forced.current = false;
        if (!cancelled) setState(s);
      } catch {
        // A failed heartbeat is not a lock held by someone else.
      }
    };
    void beat();
    const t = setInterval(() => void beat(), HEARTBEAT_MS);
    const onUnload = () => void api.unlockPost(postId);
    window.addEventListener("pagehide", onUnload);
    return () => {
      cancelled = true;
      clearInterval(t);
      window.removeEventListener("pagehide", onUnload);
      if (state?.mine !== false) void api.unlockPost(postId);
    };
    // The lock follows the post, not the state it reports.
  }, [postId]);

  return {
    heldByOther: state !== null && !state.mine,
    holderName: state?.holder_name ?? null,
    seenAgoSecs: state?.seen_ago_secs ?? null,
    takeOver: () => {
      if (postId === undefined) return;
      forced.current = true;
      void api.lockPost(postId, true).then((s) => {
        forced.current = false;
        setState(s);
      });
    },
  };
}
