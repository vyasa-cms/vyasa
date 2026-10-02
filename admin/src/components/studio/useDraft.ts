import * as React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import {
  applyOps,
  getDraft,
  previewCandidate,
  vocabulary,
  TEMPLATE_TYPES,
  type Draft,
  type Layout,
  type Section,
  type StudioOp,
  type TemplateType,
  type ThemeAssets,
  type TokenSet,
} from "@/api/themes";

/** The documents as the editor holds them. */
export interface Working {
  tokens: TokenSet;
  layout: Layout;
  templates: Record<string, string>;
  /** The theme's own stylesheet and script. */
  assets: ThemeAssets;
}

/** How long after the last edit a revision is written. */
export const SYNC_DELAY_MS = 600;
/** How long after the last edit the preview re-renders. */
export const PREVIEW_DELAY_MS = 350;

export type SaveState = "saved" | "saving" | "error";

const same = (a: unknown, b: unknown): boolean =>
  JSON.stringify(a) === JSON.stringify(b);

function fromDraft(draft: Draft): Working {
  return {
    tokens: draft.tokens,
    layout: draft.layout,
    templates: draft.templates,
    // A theme stored before assets existed has none, which is not the
    // same as having empty ones — both read as empty here.
    assets: { css: draft.assets?.css ?? "", js: draft.assets?.js ?? "" },
  };
}

/**
 * The operations that turn `synced` into `working` — one per section that
 * changed, so a revision reads "changed colors.primary.light" rather than
 * "replaced everything".
 */
export function diffOps(synced: Working, working: Working): StudioOp[] {
  const ops: StudioOp[] = [];
  if (!same(synced.tokens, working.tokens)) {
    ops.push({ op: "set_tokens", tokens: working.tokens });
  }
  for (const template of TEMPLATE_TYPES) {
    if (!same(synced.layout[template], working.layout[template])) {
      ops.push({ op: "set_layout", template, blocks: working.layout[template] });
    }
  }
  const names = new Set([
    ...Object.keys(synced.templates),
    ...Object.keys(working.templates),
  ]);
  for (const name of names) {
    const before = synced.templates[name];
    const after = working.templates[name];
    if (after === undefined) {
      if (before !== undefined) ops.push({ op: "remove_template", name });
    } else if (before !== after) {
      ops.push({ op: "set_template", name, source: after });
    }
  }
  if (!same(synced.assets, working.assets)) {
    ops.push({
      op: "set_assets",
      css: working.assets.css,
      js: working.assets.js,
    });
  }
  return ops;
}

/**
 * Replay the author's edits onto a newer remote copy.
 *
 * Every section the author changed since `base` keeps their version; every
 * section they left alone takes the remote one, so a concurrent change by
 * the assistant or another tab to a different section is adopted rather
 * than overwritten.
 */
export function rebaseWorking(base: Working, working: Working, remote: Working): Working {
  const pick = <T,>(b: T, w: T, r: T): T => (same(b, w) ? r : w);
  const layout = { ...remote.layout };
  for (const template of TEMPLATE_TYPES) {
    layout[template] = pick(base.layout[template], working.layout[template], remote.layout[template]);
  }
  const templates: Record<string, string> = { ...remote.templates };
  const names = new Set([...Object.keys(base.templates), ...Object.keys(working.templates)]);
  for (const name of names) {
    const before = base.templates[name];
    const after = working.templates[name];
    if (before === after) continue;
    if (after === undefined) delete templates[name];
    else templates[name] = after;
  }
  return {
    tokens: pick(base.tokens, working.tokens, remote.tokens),
    layout,
    templates,
    assets: pick(base.assets, working.assets, remote.assets),
  };
}

/** Raised by `flush` while a remote conflict is waiting for a decision. */
export const CONFLICT_MESSAGE =
  "The draft changed elsewhere. Choose whether to keep your edits or take the newer version before saving.";

/**
 * A draft open in the studio.
 *
 * Edits land in `working` immediately. A short while after the last edit
 * the difference from what the server has is sent as one batch of
 * operations and becomes a revision; the preview re-renders from
 * `working` on its own, shorter delay, so a colour being nudged shows up
 * before it is saved. A rejected batch leaves `working` alone and reports
 * why, so the author can fix the field rather than lose the edit.
 */
export function useDraft(draftId: string) {
  const queryClient = useQueryClient();
  const draftQuery = useQuery({
    queryKey: ["theme-draft", draftId],
    queryFn: () => getDraft(draftId),
    // While the assistant works, watch for it to finish.
    refetchInterval: (query) =>
      query.state.data?.status === "generating" ? 2000 : false,
  });
  const vocabQuery = useQuery({
    queryKey: ["theme-vocabulary"],
    queryFn: vocabulary,
    staleTime: Infinity,
  });

  const [working, setWorking] = React.useState<Working | null>(null);
  const workingRef = React.useRef<Working | null>(null);
  workingRef.current = working;
  const inFlight = React.useRef<Promise<void> | null>(null);
  const syncedRef = React.useRef<Working | null>(null);
  const seenRevision = React.useRef<number>(0);
  const [saveState, setSaveState] = React.useState<SaveState>("saved");
  const [saveError, setSaveError] = React.useState<string | null>(null);
  // A newer remote revision that arrived while local edits were unsaved.
  // Autosave is paused until the author picks a side.
  const [conflict, setConflictState] = React.useState<Draft | null>(null);
  const conflictRef = React.useRef<Draft | null>(null);
  const setConflict = React.useCallback((draft: Draft | null) => {
    conflictRef.current = draft;
    setConflictState(draft);
  }, []);

  const [previewPath, setPreviewPath] = React.useState("/");
  const [previewHtml, setPreviewHtml] = React.useState<string | null>(null);
  const [previewError, setPreviewError] = React.useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = React.useState(false);
  const previewSeq = React.useRef(0);

  // First load, and any time the server state is replaced wholesale (a
  // revert, the assistant finishing): adopt it as the working copy.
  const reset = React.useCallback(
    (draft: Draft) => {
      seenRevision.current = Math.max(seenRevision.current, draft.revision);
      const next = fromDraft(draft);
      syncedRef.current = next;
      workingRef.current = next;
      setWorking(next);
      setSaveState("saved");
      setSaveError(null);
      setConflict(null);
      queryClient.setQueryData(["theme-draft", draftId], draft);
    },
    [draftId, queryClient, setConflict],
  );

  const loadedId = React.useRef<string | null>(null);
  React.useEffect(() => {
    const draft = draftQuery.data;
    if (draft === undefined) return;
    if (loadedId.current !== draft.id) {
      loadedId.current = draft.id;
      seenRevision.current = draft.revision;
      reset(draft);
      return;
    }
    // Adopt remote revisions only while clean. With local edits pending,
    // stop autosaving (it would overwrite the newer copy with stale
    // sections) and let the author choose.
    if (draft.revision > seenRevision.current) {
      if (workingRef.current && syncedRef.current && !same(workingRef.current, syncedRef.current)) {
        if ((conflictRef.current?.revision ?? 0) < draft.revision) setConflict(draft);
        return;
      }
      seenRevision.current = draft.revision;
      reset(draft);
    }
  }, [draftQuery.data, reset, setConflict]);

  const isDirty = React.useCallback(() => workingRef.current !== null && syncedRef.current !== null && !same(workingRef.current, syncedRef.current), []);

  // Serialize writes, including changes made while a prior write is in flight.
  const flush = React.useCallback(async (): Promise<void> => {
    // Wait out whatever is in flight. A failed write is not this caller's
    // failure: the loop below retries with the current changes once. Only
    // one waiter starts the next batch; the rest see it and wait on it.
    while (inFlight.current) {
      const current = inFlight.current;
      try { await current; } catch { /* retried below */ }
      if (inFlight.current === current) inFlight.current = null;
    }
    if (conflictRef.current) throw new Error(CONFLICT_MESSAGE);
    if (!isDirty()) return;
    const run = async () => {
      try {
        setSaveState("saving");
        while (isDirty() && !conflictRef.current) {
          const sent = workingRef.current!;
          const draft = await applyOps(draftId, diffOps(syncedRef.current!, sent));
          syncedRef.current = sent;
          seenRevision.current = Math.max(seenRevision.current, draft.revision);
          queryClient.setQueryData(["theme-draft", draftId], draft);
        }
        if (conflictRef.current) throw new Error(CONFLICT_MESSAGE);
        setSaveState("saved");
        setSaveError(null);
      } catch (error) {
        setSaveState("error");
        if (!conflictRef.current) setSaveError(error instanceof Error ? error.message : String(error));
        throw error;
      }
    };
    const pending = run();
    inFlight.current = pending;
    try { await pending; } finally { if (inFlight.current === pending) inFlight.current = null; }
  }, [draftId, queryClient, isDirty]);

  React.useEffect(() => {
    // Paused while a conflict waits for the author's decision.
    if (conflict !== null || !isDirty()) return;
    const timer = setTimeout(() => { void flush().catch(() => undefined); }, SYNC_DELAY_MS);
    return () => clearTimeout(timer);
  }, [working, conflict, flush, isDirty]);

  /**
   * Keep mine: take the newer remote copy as the base and replay only the
   * sections this author changed on top of it. Autosave then sends just
   * those sections.
   */
  const keepMine = React.useCallback(() => {
    const remote = conflictRef.current;
    if (remote === null || workingRef.current === null || syncedRef.current === null) return;
    const base = fromDraft(remote);
    const next = rebaseWorking(syncedRef.current, workingRef.current, base);
    seenRevision.current = Math.max(seenRevision.current, remote.revision);
    syncedRef.current = base;
    workingRef.current = next;
    setConflict(null);
    setSaveError(null);
    setSaveState("saved");
    setWorking(next);
  }, [setConflict]);

  /** Take theirs: drop local edits and adopt the newer remote copy. */
  const takeTheirs = React.useCallback(() => {
    const remote = conflictRef.current;
    if (remote !== null) reset(remote);
  }, [reset]);

  // Re-render the preview from the working copy.
  React.useEffect(() => {
    if (working === null) return;
    const seq = ++previewSeq.current;
    setPreviewLoading(true);
    const timer = setTimeout(() => {
      previewCandidate({ ...working, path: previewPath, base_theme_id: draftQuery.data?.base_theme_id })
        .then((html) => {
          if (seq !== previewSeq.current) return;
          setPreviewHtml(html);
          setPreviewError(null);
        })
        .catch((e: unknown) => {
          if (seq !== previewSeq.current) return;
          setPreviewError(e instanceof Error ? e.message : String(e));
        })
        .finally(() => {
          if (seq === previewSeq.current) setPreviewLoading(false);
        });
    }, PREVIEW_DELAY_MS);
    return () => clearTimeout(timer);
  }, [working, previewPath, draftQuery.data?.base_theme_id]);

  const update = React.useCallback((fn: (w: Working) => Working) => {
    setWorking((w) => (w === null ? w : fn(w)));
  }, []);

  const setTokens = React.useCallback(
    (tokens: TokenSet) => update((w) => ({ ...w, tokens })),
    [update],
  );
  const setLayout = React.useCallback(
    (template: TemplateType, blocks: Section[]) =>
      update((w) => ({ ...w, layout: { ...w.layout, [template]: blocks } })),
    [update],
  );
  const setTemplate = React.useCallback(
    (name: string, source: string) =>
      update((w) => ({ ...w, templates: { ...w.templates, [name]: source } })),
    [update],
  );
  const setAssets = React.useCallback(
    (assets: ThemeAssets) => update((w) => ({ ...w, assets })),
    [update],
  );
  const removeTemplate = React.useCallback(
    (name: string) =>
      update((w) => {
        const templates = { ...w.templates };
        delete templates[name];
        return { ...w, templates };
      }),
    [update],
  );

  /** Drop unsaved edits and go back to what the server has. */
  const discard = React.useCallback(() => {
    const draft = conflictRef.current ?? queryClient.getQueryData<Draft>(["theme-draft", draftId]);
    if (draft !== undefined) reset(draft);
  }, [draftId, queryClient, reset]);

  return {
    draft: draftQuery.data,
    isLoading: draftQuery.isPending || vocabQuery.isPending,
    loadError: draftQuery.error ?? vocabQuery.error,
    vocab: vocabQuery.data,
    working,
    setTokens,
    setLayout,
    setTemplate,
    setAssets,
    removeTemplate,
    saveState: conflict !== null ? "error" as const : saveState === "saved" && isDirty() ? "saving" as const : saveState,
    conflict,
    keepMine,
    takeTheirs,
    isDirty,
    flush,
    saveError,
    discard,
    reset,
    previewPath,
    setPreviewPath,
    previewHtml,
    previewError,
    previewLoading,
  };
}

export type DraftHandle = ReturnType<typeof useDraft>;
