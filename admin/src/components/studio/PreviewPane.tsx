import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { ExternalLink, Monitor, Moon, Smartphone, Sun, Tablet, X } from "lucide-react";

import { api } from "@/api/client";
import type { TokenSet } from "@/api/themes";
import { previewWidths, type PreviewWidth } from "@/lib/preview-widths";
import { PREVIEW_SANDBOX, useCanvasBridge } from "./useCanvasBridge";
import { cn } from "@/lib/utils";

const ICONS = { desktop: Monitor, tablet: Tablet, phone: Smartphone } as const;

type ViewportKey = PreviewWidth["key"];


function ToggleButton({
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
          ? "bg-background text-foreground shadow-sm"
          : "text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

/** Pages worth previewing: fixed routes plus a few real posts and pages. */
function usePageOptions(): { value: string; label: string }[] {
  const posts = useQuery({
    queryKey: ["posts", "preview-options"],
    queryFn: () => api.listPosts({ status: "published", per_page: 12, page: 1 }),
    staleTime: 60_000,
  });
  const items = posts.data?.items ?? [];
  const options: { value: string; label: string }[] = [{ value: "/", label: "Home" }];
  for (const p of items) {
    const isPage = p.type === "page";
    options.push({
      value: p.public_url ?? (isPage ? `/${p.slug}` : `/post/${p.slug}`),
      label: `${isPage ? "Page" : "Post"}: ${p.title || "(untitled)"}`,
    });
  }
  options.push({ value: "/search?s=the", label: "Search results" });
  options.push({ value: "/studio-preview-missing-page", label: "Not found (404)" });
  return options;
}

/**
 * The size of the area the artboard is floating in.
 *
 * Needed to scale a 1280px desktop artboard down into a pane that is not
 * 1280px wide. A media query cannot answer this: the pane is the window
 * minus the admin's navigation minus the two rails beside it.
 */
function useFieldSize(): [React.RefObject<HTMLDivElement>, { w: number; h: number } | null] {
  const ref = React.useRef<HTMLDivElement>(null);
  const [size, setSize] = React.useState<{ w: number; h: number } | null>(null);
  React.useLayoutEffect(() => {
    const el = ref.current;
    if (el === null) return undefined;
    const read = () => {
      const r = el.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) setSize({ w: r.width, h: r.height });
    };
    read();
    if (typeof ResizeObserver !== "function") return undefined;
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, size];
}

export function PreviewPane({
  html,
  error,
  loading,
  path,
  onPathChange,
  openUrl,
  title,
  tokens,
  selectedId = null,
  onSelect,
  onMove,
  onInlineEdit,
  onDelete,
  onInsertKind,
}: {
  html: string | null;
  error: string | null;
  loading: boolean;
  path: string;
  onPathChange: (path: string) => void;
  openUrl: string;
  title: string;
  /** The candidate's tokens, so the preview sizes match its breakpoints. */
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
  /** Called when an insert-library card is dropped on the page. */
  onInsertKind?: (kind: string, targetId: string | null, place: "before" | "after") => void;
}) {
  const [dark, setDark] = React.useState(false);
  const [viewport, setViewport] = React.useState<ViewportKey>("desktop");
  const iframeRef = React.useRef<HTMLIFrameElement>(null);
  const options = usePageOptions();
  const [fieldRef, field] = useFieldSize();
  const { onFrameLoad } = useCanvasBridge({
    frameRef: iframeRef,
    html,
    dark,
    selectedId,
    onSelect,
    onMove,
    onInlineEdit,
    onDelete,
    onInsertKind,
  });

  const viewports = previewWidths(tokens);
  const current = viewports.find((v) => v.key === viewport);

  // Only ever scale down: a phone artboard blown up to fill the pane would
  // be a lie about what the visitor sees.
  const target = current?.px ?? null;
  const scale =
    target === null || field === null ? 1 : Math.min(1, (field.w - 32) / target);
  const frameWidth = target === null ? "100%" : `${target}px`;
  const frameHeight = field === null ? "100%" : `${Math.max(340, field.h - 32) / scale}px`;

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border bg-card">
      <div className="flex flex-wrap items-center gap-2 border-b px-3 py-2">
        <select
          aria-label="Page to preview"
          value={path}
          onChange={(e) => onPathChange(e.target.value)}
          className="h-7 max-w-[14rem] rounded-md border bg-background px-2 text-xs"
        >
          {options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>

        <div className="flex gap-0.5 rounded-md border bg-muted/60 p-0.5">
          <ToggleButton active={!dark} onClick={() => setDark(false)} label="Light">
            <Sun className="h-3.5 w-3.5" aria-hidden="true" />
          </ToggleButton>
          <ToggleButton active={dark} onClick={() => setDark(true)} label="Dark">
            <Moon className="h-3.5 w-3.5" aria-hidden="true" />
          </ToggleButton>
        </div>

        <div className="flex gap-0.5 rounded-md border bg-muted/60 p-0.5">
          {viewports.map((v) => {
            const Icon = ICONS[v.key];
            return (
              <ToggleButton
                key={v.key}
                active={viewport === v.key}
                onClick={() => setViewport(v.key)}
                label={v.px === null ? v.label : `${v.label} — ${v.px}px`}
              >
                <Icon className="h-3.5 w-3.5" aria-hidden="true" />
              </ToggleButton>
            );
          })}
        </div>

        <span className="ml-auto flex items-center gap-2 text-[11px] text-muted-foreground">
          {loading ? (
            <span className="inline-flex items-center gap-1">
              <span className="h-2.5 w-2.5 animate-spin rounded-full border-2 border-muted border-t-primary" />
              Rendering
            </span>
          ) : null}
          <a
            href={openUrl}
            target="_blank"
            rel="noreferrer"
            className="inline-flex h-7 items-center gap-1 rounded-md border px-2 hover:bg-accent"
          >
            <ExternalLink className="h-3 w-3" aria-hidden="true" />
            <span className="hidden sm:inline">Open</span>
          </a>
        </span>
      </div>

      {error !== null ? (
        <div
          role="alert"
          className="border-b bg-destructive/10 px-3 py-2 font-mono text-[11px] text-destructive"
        >
          Preview paused: {error}
        </div>
      ) : null}

      {/* The field the artboard floats on. Dotted, recessed and clickable:
          clicking past the page is how you let go of a selection. */}
      <div
        ref={fieldRef}
        onClick={() => onSelect?.(null)}
        className="relative flex min-h-[340px] flex-1 justify-center overflow-auto bg-muted/50 p-4"
        style={{
          backgroundImage:
            "radial-gradient(circle, rgba(120,120,130,.18) 1px, transparent 1px)",
          backgroundSize: "16px 16px",
        }}
      >
        <div style={{ width: target === null ? "100%" : `${target * scale}px` }}>
          <iframe
            ref={iframeRef}
            srcDoc={html ?? "<!doctype html><title>Preview</title>"}
            // Theme and plugin markup is untrusted: no scripts may run with the
            // admin's origin. Same-origin stays so the canvas bridge can wire
            // listeners from this (parent) document through contentDocument.
            sandbox={PREVIEW_SANDBOX}
            title={`Preview of ${title}`}
            onLoad={onFrameLoad}
            style={{
              width: frameWidth,
              height: frameHeight,
              transform: scale === 1 ? undefined : `scale(${scale})`,
              transformOrigin: "top left",
            }}
            className="min-h-[340px] rounded-md bg-white shadow-[0_10px_40px_-12px_rgba(0,0,0,.35)] ring-1 ring-black/10 transition-[width] duration-200"
            data-testid="preview-frame"
          />
        </div>
      </div>

      <div className="flex items-center gap-2 border-t px-3 py-1.5 text-[11px] text-muted-foreground">
        <span>
          {current?.label ?? "Desktop"}
          {target === null ? "" : ` · ${target}px`}
          {scale === 1 ? "" : ` · ${Math.round(scale * 100)}%`}
        </span>
        {selectedId === null ? (
          <span className="ml-auto">Click a section to edit it · drag to rearrange</span>
        ) : (
          <span className="ml-auto inline-flex items-center gap-1.5">
            <span className="h-2 w-2 rounded-full bg-[rgb(99,102,241)]" aria-hidden="true" />
            <span className="font-medium text-foreground" data-testid="canvas-selection">
              {selectedId}
            </span>
            <button
              type="button"
              onClick={() => onSelect?.(null)}
              aria-label="Clear selection"
              className="rounded p-0.5 hover:bg-accent hover:text-foreground"
            >
              <X className="h-3 w-3" aria-hidden="true" />
            </button>
          </span>
        )}
      </div>
    </div>
  );
}
