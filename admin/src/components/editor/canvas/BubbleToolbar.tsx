import * as React from "react";
import type { Editor } from "@tiptap/core";
import { BubbleMenu } from "@tiptap/react/menus";
import { useEditorState } from "@tiptap/react";
import {
  Bold,
  Code,
  ExternalLink,
  Highlighter,
  Italic,
  Link2,
  Link2Off,
  Pencil,
  Sparkles,
  Strikethrough,
  Subscript,
  Superscript,
  Underline,
} from "lucide-react";
import { LINK_PROMPT_EVENT } from "./extensions";
import { useEditorDom } from "./useEditorDom";
import { runSelectionAssist, SELECTION_ASSISTS, type TextAssistKind } from "./ai";
import { useAiAvailable } from "../useAiAvailable";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/client";
import { cn } from "@/lib/utils";

type Mode = "format" | "link" | "assist";

// Stable across renders on purpose: BubbleMenu rebuilds its ProseMirror
// plugin whenever these props change identity, and a fresh object literal
// on every render sends it into an update loop.
const MENU_OPTIONS = { placement: "top" as const };
const shouldShow = ({
  editor: e,
  from,
  to,
}: {
  editor: Editor;
  from: number;
  to: number;
}): boolean => {
  if (e.isActive("codeBlock") || e.isActive("rpBlock") || e.isActive("vyImage")) {
    return false;
  }
  if (from !== to) return true;
  return e.isActive("link");
};

/**
 * Formatting for the current selection, plus a link card when the caret
 * merely rests inside a link, plus the writing assistant.
 *
 * Every mark here maps onto a tag in the server's inline allowlist
 * (`vyasa_core::block::INLINE_TAGS`); the schema test pins the set.
 */
export function BubbleToolbar({ editor }: { editor: Editor }) {
  const [mode, setMode] = React.useState<Mode>("format");
  const [href, setHref] = React.useState("");
  const [busy, setBusy] = React.useState<TextAssistKind | null>(null);
  const ai = useAiAvailable();

  const state = useEditorState({
    editor,
    selector: ({ editor: e }) => ({
      empty: e.state.selection.empty,
      bold: e.isActive("bold"),
      italic: e.isActive("italic"),
      underline: e.isActive("underline"),
      strike: e.isActive("strike"),
      code: e.isActive("code"),
      highlight: e.isActive("highlight"),
      subscript: e.isActive("subscript"),
      superscript: e.isActive("superscript"),
      link: e.isActive("link"),
      linkHref: String(e.getAttributes("link")["href"] ?? ""),
      selectedWords: e.state.selection.empty
        ? 0
        : e.state.doc
            .textBetween(e.state.selection.from, e.state.selection.to, " ", " ")
            .trim()
            .split(/\s+/)
            .filter((w) => w !== "").length,
    }),
  });

  // Typing a title instead of a URL offers the site's own entries.
  const [linkQuery, setLinkQuery] = React.useState("");
  React.useEffect(() => {
    const t = setTimeout(() => setLinkQuery(href.trim()), 200);
    return () => clearTimeout(t);
  }, [href]);
  const isUrlLike = /^(https?:|mailto:|\/|#)/.test(linkQuery) || linkQuery.includes(".");
  const matches = useQuery({
    queryKey: ["link-picker", linkQuery],
    queryFn: () => api.listPosts({ search: linkQuery, per_page: 6, status: "published" }),
    enabled: mode === "link" && linkQuery.length >= 2 && !isUrlLike,
    staleTime: 30_000,
  });
  const pickEntry = (p: { slug: string; type: string }) => {
    const url = p.type === "page" ? `/${p.slug}` : p.type === "post" ? `/post/${p.slug}` : `/${p.type}/${p.slug}`;
    editor.chain().focus().extendMarkRange("link").setLink({ href: url }).run();
    setMode("format");
    setHref("");
  };

  // Ctrl/Cmd+K (and the phone toolbar) raise this after widening the selection.
  const dom = useEditorDom(editor);
  React.useEffect(() => {
    if (dom === null) return;
    const open = () => {
      setHref(String(editor.getAttributes("link")["href"] ?? ""));
      setMode("link");
    };
    dom.addEventListener(LINK_PROMPT_EVENT, open);
    return () => dom.removeEventListener(LINK_PROMPT_EVENT, open);
  }, [editor, dom]);

  // A fresh selection starts from the formatting row again.
  React.useEffect(() => {
    if (mode !== "format" && busy === null) setMode("format");
  }, [state.empty, state.linkHref]);

  const applyLink = () => {
    const url = href.trim();
    if (url === "") {
      editor.chain().focus().extendMarkRange("link").unsetLink().run();
    } else {
      // Bare domains would otherwise be treated as relative paths.
      const normalised = /^(https?:|mailto:|\/|#)/.test(url)
        ? url
        : `https://${url}`;
      editor.chain().focus().extendMarkRange("link").setLink({ href: normalised }).run();
    }
    setMode("format");
    setHref("");
  };

  const assist = async (kind: TextAssistKind) => {
    setBusy(kind);
    try {
      await runSelectionAssist(editor, kind);
    } finally {
      setBusy(null);
      setMode("format");
    }
  };

  const caretInLink = state.empty && state.link;

  return (
    <BubbleMenu editor={editor} options={MENU_OPTIONS} shouldShow={shouldShow}>
      <div
        className="flex max-w-[calc(100vw-2rem)] items-center gap-0.5 rounded-lg border bg-popover p-1 shadow-lg"
        data-testid="bubble-toolbar"
      >
        {busy !== null ? (
          <span className="inline-flex h-7 items-center gap-1.5 px-2 text-xs text-muted-foreground">
            <Sparkles className="h-3.5 w-3.5 animate-pulse text-primary" aria-hidden="true" />
            {SELECTION_ASSISTS.find((a) => a.kind === busy)?.label ?? "Working"}…
          </span>
        ) : mode === "link" ? (
          <form
            className="relative flex items-center gap-1"
            onSubmit={(e) => {
              e.preventDefault();
              applyLink();
            }}
          >
            {(matches.data?.items ?? []).length > 0 ? (
              <ul
                className="absolute left-0 top-full z-50 mt-1 w-72 overflow-hidden rounded-md border bg-popover shadow-lg"
                role="listbox"
                aria-label="Your entries"
                data-testid="link-picker"
              >
                {(matches.data?.items ?? []).map((p) => (
                  <li key={p.id}>
                    <button
                      type="button"
                      role="option"
                      aria-selected={false}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => pickEntry(p)}
                      className="flex w-full flex-col px-2 py-1.5 text-left hover:bg-accent"
                    >
                      <span className="truncate text-sm">{p.title}</span>
                      <span className="truncate text-[11px] text-muted-foreground">
                        {p.type === "page" ? `/${p.slug}` : `/post/${p.slug}`}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            ) : null}
            <input
              autoFocus
              value={href}
              onChange={(e) => setHref(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  setMode("format");
                  editor.commands.focus();
                }
              }}
              placeholder="Link, or the title of a page or post"
              aria-label="Link address"
              className="h-7 w-56 rounded-md border border-input bg-background px-2 text-sm focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
            />
            <button
              type="submit"
              className="h-7 rounded-md bg-primary px-2 text-xs font-medium text-primary-foreground"
            >
              {href.trim() === "" && state.link ? "Remove" : "Apply"}
            </button>
          </form>
        ) : mode === "assist" ? (
          <div className="flex items-center gap-0.5" role="menu" aria-label="Writing assistant">
            {SELECTION_ASSISTS.map((a) => (
              <button
                key={a.kind}
                type="button"
                role="menuitem"
                title={a.hint}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => void assist(a.kind)}
                className="h-7 rounded-md px-2 text-xs font-medium text-foreground hover:bg-accent"
              >
                {a.label}
              </button>
            ))}
            <button
              type="button"
              aria-label="Back to formatting"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setMode("format")}
              className="h-7 rounded-md px-2 text-xs text-muted-foreground hover:bg-accent"
            >
              ×
            </button>
          </div>
        ) : caretInLink ? (
          <div className="flex items-center gap-1 pl-1.5">
            <a
              href={state.linkHref}
              target="_blank"
              rel="noopener noreferrer"
              className="max-w-[16rem] truncate text-xs text-primary underline underline-offset-2"
              title={state.linkHref}
            >
              {state.linkHref}
            </a>
            <ToolButton
              label="Edit link"
              active={false}
              onClick={() => {
                setHref(state.linkHref);
                editor.chain().focus().extendMarkRange("link").run();
                setMode("link");
              }}
            >
              <Pencil className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>
            <ToolButton
              label="Open link in a new tab"
              active={false}
              onClick={() => window.open(state.linkHref, "_blank", "noopener")}
            >
              <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>
            <ToolButton
              label="Remove link"
              active={false}
              onClick={() => editor.chain().focus().extendMarkRange("link").unsetLink().run()}
            >
              <Link2Off className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>
          </div>
        ) : (
          <>
            {state.selectedWords > 0 ? (
              <span className="px-1.5 text-[11px] tabular-nums text-muted-foreground" data-testid="selection-words">
                {state.selectedWords}w
              </span>
            ) : null}
            <ToolButton label="Bold" shortcut="Ctrl+B" active={state.bold} onClick={() => editor.chain().focus().toggleBold().run()}>
              <Bold className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>
            <ToolButton label="Italic" shortcut="Ctrl+I" active={state.italic} onClick={() => editor.chain().focus().toggleItalic().run()}>
              <Italic className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>
            <ToolButton label="Underline" shortcut="Ctrl+U" active={state.underline} onClick={() => editor.chain().focus().toggleUnderline().run()}>
              <Underline className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>
            <ToolButton label="Strikethrough" shortcut="Ctrl+Shift+S" active={state.strike} onClick={() => editor.chain().focus().toggleStrike().run()}>
              <Strikethrough className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>

            <Divider />

            <ToolButton label="Inline code" shortcut="Ctrl+E" active={state.code} onClick={() => editor.chain().focus().toggleCode().run()}>
              <Code className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>
            <ToolButton label="Highlight" shortcut="Ctrl+Shift+H" active={state.highlight} onClick={() => editor.chain().focus().toggleHighlight().run()}>
              <Highlighter className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>
            <ToolButton label="Subscript" shortcut="Ctrl+," active={state.subscript} onClick={() => editor.chain().focus().toggleSubscript().run()}>
              <Subscript className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>
            <ToolButton label="Superscript" shortcut="Ctrl+." active={state.superscript} onClick={() => editor.chain().focus().toggleSuperscript().run()}>
              <Superscript className="h-3.5 w-3.5" aria-hidden="true" />
            </ToolButton>

            <Divider />

            {state.link ? (
              <ToolButton
                label="Remove link"
                active
                onClick={() => editor.chain().focus().extendMarkRange("link").unsetLink().run()}
              >
                <Link2Off className="h-3.5 w-3.5" aria-hidden="true" />
              </ToolButton>
            ) : (
              <ToolButton
                label="Add link"
                shortcut="Ctrl+K"
                active={false}
                onClick={() => {
                  setHref("");
                  setMode("link");
                }}
              >
                <Link2 className="h-3.5 w-3.5" aria-hidden="true" />
              </ToolButton>
            )}

            {ai.text ? <Divider /> : null}

            {ai.text ? (
            <button
              type="button"
              aria-label="Writing assistant"
              title="Improve, shorten, expand or fix this passage"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setMode("assist")}
              className="inline-flex h-7 items-center gap-1 rounded-md px-1.5 text-xs font-medium text-primary hover:bg-accent"
            >
              <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
              Assist
            </button>
            ) : null}
          </>
        )}
      </div>
    </BubbleMenu>
  );
}

function Divider() {
  return <span className="mx-0.5 h-4 w-px bg-border" aria-hidden="true" />;
}

function ToolButton({
  label,
  shortcut,
  active,
  onClick,
  children,
}: {
  label: string;
  shortcut?: string;
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onMouseDown={(e) => e.preventDefault()} // keep the selection alive
      onClick={onClick}
      aria-label={label}
      aria-pressed={active}
      title={shortcut === undefined ? label : `${label} (${shortcut})`}
      className={cn(
        "inline-flex h-7 w-7 items-center justify-center rounded-md transition-colors",
        active
          ? "bg-primary text-primary-foreground"
          : "text-muted-foreground hover:bg-accent hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}
