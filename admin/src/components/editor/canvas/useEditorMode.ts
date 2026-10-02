import * as React from "react";

export type EditorMode = "canvas" | "classic";

const STORAGE_KEY = "vyasa-editor-mode";

/**
 * Which editor the author is using.
 *
 * The canvas is the default: it edits every common kind in place, uploads
 * images by drop or paste, and round-trips everything else untouched. The
 * classic form editor stays one click away for anyone who prefers fields,
 * and the choice persists per browser. Switching is instant in both
 * directions because the two editors share `BlockDocument v1` — no
 * conversion, no migration.
 */
export function useEditorMode(): [EditorMode, (mode: EditorMode) => void] {
  const [mode, setMode] = React.useState<EditorMode>(() => {
    if (typeof localStorage === "undefined") return "canvas";
    return localStorage.getItem(STORAGE_KEY) === "classic" ? "classic" : "canvas";
  });

  const update = React.useCallback((next: EditorMode) => {
    setMode(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // Storage unavailable — the choice just won't outlive the tab.
    }
  }, []);

  return [mode, update];
}

/** How wide the canvas column runs on a large screen. */
export type CanvasWidth = "measure" | "wide" | "full";

const WIDTH_KEY = "vyasa-canvas-width";

/** The `ch` measure each width resolves to; `full` has no cap. */
export const CANVAS_MEASURE: Record<CanvasWidth, string> = {
  measure: "70ch",
  wide: "110ch",
  full: "100%",
};

/**
 * The canvas column width, remembered per browser. A 70-character
 * measure reads best, but on an ultrawide monitor it leaves most of the
 * screen empty; the writer decides.
 */
export function useCanvasWidth(): [CanvasWidth, (w: CanvasWidth) => void] {
  const [width, setWidth] = React.useState<CanvasWidth>(() => {
    try {
      const v = window.localStorage.getItem(WIDTH_KEY);
      return v === "wide" || v === "full" ? v : "measure";
    } catch {
      return "measure";
    }
  });
  const set = React.useCallback((w: CanvasWidth) => {
    setWidth(w);
    try {
      window.localStorage.setItem(WIDTH_KEY, w);
    } catch {
      /* private mode: the choice lasts for the session */
    }
  }, []);
  return [width, set];
}
