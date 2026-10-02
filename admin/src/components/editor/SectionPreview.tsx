import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { Monitor, Moon, Smartphone, Sun, Tablet } from "lucide-react";

import { api } from "@/api/client";
import type { Section, TokenSet } from "@/api/themes";
import { PREVIEW_SANDBOX, useCanvasBridge } from "@/components/studio/useCanvasBridge";
import { previewWidths, type PreviewWidth } from "@/lib/preview-widths";
import { cn } from "@/lib/utils";

const ICONS = { desktop: Monitor, tablet: Tablet, phone: Smartphone } as const;

function Toggle({
  active,
  onClick,
  label,
  children,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      aria-label={label}
      title={label}
      className={cn(
        "inline-flex h-6 w-7 items-center justify-center rounded transition-colors",
        active
          ? "bg-background text-foreground shadow-xs"
          : "text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

/** Waits for edits to stop before asking the server to render. */
function useSettled<T>(value: T, ms: number): T {
  const [settled, setSettled] = React.useState(value);
  React.useEffect(() => {
    const t = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return settled;
}

/**
 * The page, as a visitor would see it, beside the tree that describes it.
 *
 * Composing without this is done blind: the tree says `hero`, and whether
 * that is the right hero is a question only the rendered page answers. It
 * renders the *unsaved* state, so the loop is edit → look, not edit → save
 * → open a tab → look.
 */
export function SectionPreview({
  postId,
  sections,
  document: doc,
  tokens,
  selectedId = null,
  onSelect,
  onMove,
  onInlineEdit,
  onDelete,
}: {
  postId: string;
  sections: Section[];
  document: unknown;
  tokens: TokenSet;
  /** Section id currently selected, outlined on the canvas. */
  selectedId?: string | null;
  /** Called when a section on the canvas is picked, or the canvas cleared. */
  onSelect?: (id: string | null) => void;
  /** Called when a section is dragged to a new place on the page. */
  onMove?: (id: string, targetId: string, place: "before" | "after") => void;
  /** Called when text marked `data-vy-edit` is edited in place. */
  onInlineEdit?: (id: string, key: string, value: string) => void;
  /** Called when the keyboard asks for the selected section's removal. */
  onDelete?: (id: string) => void;
}) {
  const [dark, setDark] = React.useState(false);
  const [viewport, setViewport] = React.useState<PreviewWidth["key"]>("desktop");
  const frameRef = React.useRef<HTMLIFrameElement>(null);

  // The tree changes on every keystroke in a settings field; rendering each
  // one would be a request per character.
  const settled = useSettled(JSON.stringify({ sections, doc }), 500);

  const preview = useQuery({
    queryKey: ["page-preview", postId, settled],
    // Render exactly what the key names. Reading the live props here would
    // cache a newer tree under an older key (a refetch of a settled key
    // fires before the debounce catches up).
    queryFn: () => {
      const keyed = JSON.parse(settled) as { sections: Section[]; doc?: unknown };
      return api.renderPage(postId, { sections: keyed.sections, content: keyed.doc });
    },
    retry: false,
    // Keep the last good render on screen while the next one is in flight,
    // so the pane does not blank out on every edit.
    placeholderData: (prev) => prev,
  });

  const { onFrameLoad } = useCanvasBridge({
    frameRef,
    html: preview.data,
    dark,
    selectedId,
    onSelect,
    onMove,
    onInlineEdit,
    onDelete,
  });

  const sizes = previewWidths(tokens);
  const width = sizes.find((v) => v.key === viewport)?.width ?? "100%";
  const message = preview.error === null ? null : String(preview.error.message);

  return (
    <div
      className="flex min-h-96 flex-col overflow-hidden rounded-lg border bg-muted/40"
      data-testid="section-preview"
    >
      <div className="flex flex-wrap items-center gap-2 border-b bg-card px-2 py-1.5">
        <span className="text-[11px] font-medium">Preview</span>

        <div className="flex gap-0.5 rounded-md border bg-muted/60 p-0.5">
          <Toggle active={!dark} onClick={() => setDark(false)} label="Light">
            <Sun className="h-3.5 w-3.5" aria-hidden="true" />
          </Toggle>
          <Toggle active={dark} onClick={() => setDark(true)} label="Dark">
            <Moon className="h-3.5 w-3.5" aria-hidden="true" />
          </Toggle>
        </div>

        <div className="flex gap-0.5 rounded-md border bg-muted/60 p-0.5">
          {sizes.map((v) => {
            const Icon = ICONS[v.key];
            return (
              <Toggle
                key={v.key}
                active={viewport === v.key}
                onClick={() => setViewport(v.key)}
                label={v.px === null ? v.label : `${v.label} — ${v.px}px`}
              >
                <Icon className="h-3.5 w-3.5" aria-hidden="true" />
              </Toggle>
            );
          })}
        </div>

        {preview.isFetching ? (
          <span className="ml-auto inline-flex items-center gap-1 text-[11px] text-muted-foreground">
            <span className="h-2.5 w-2.5 animate-spin rounded-full border-2 border-muted border-t-primary" />
            Rendering
          </span>
        ) : null}
      </div>

      {message !== null ? (
        <div
          role="alert"
          className="border-b bg-destructive/10 px-2 py-1.5 font-mono text-[11px] text-destructive"
        >
          {message}
        </div>
      ) : null}

      <div className="flex min-h-88 flex-1 justify-center overflow-auto p-2">
        <iframe
          ref={frameRef}
          srcDoc={preview.data ?? "<!doctype html><title>Preview</title>"}
          // Theme and plugin markup is untrusted: no scripts may run with the
          // admin's origin. Same-origin stays so the canvas bridge can wire
          // listeners from this (parent) document through contentDocument.
          sandbox={PREVIEW_SANDBOX}
          title="Preview of this page"
          onLoad={onFrameLoad}
          style={{ width }}
          className="h-full min-h-88 rounded-md border bg-white shadow-xs transition-[width] duration-200"
          data-testid="page-preview-frame"
        />
      </div>
    </div>
  );
}
