import { useMe } from "@/components/auth";
import { useCapabilities } from "@/lib/capabilities";
import { postPath } from "@/lib/permalink";
import * as React from "react";
import { Link, useBlocker, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Globe,
  AlertTriangle,
  ArrowLeft,
  Check,
  Cloud,
  CloudOff,
  Code2,
  ExternalLink,
  GitCompare,
  History,
  ListTree,
  Maximize2,
  Minimize2,
  PanelRightClose,
  PanelRightOpen,
  LayoutTemplate,
  PenLine,
  Rows3,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import { api, ApiError, type FieldValues, type PostResponse } from "@/api/client";
import { fieldErrorsOf } from "@/lib/fields";
import type { Section } from "@/api/themes";
import { Button } from "@/components/ui/button";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, ErrorNote, StatusChip } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { formatRelative } from "@/routes/_auth/index";
import { cn } from "@/lib/utils";
import { useMediaQuery } from "@/lib/use-media-query";
import { BlockEditor } from "./BlockEditor";
import { PageSections } from "./PageSections";
import { useCanvasWidth, useEditorMode, CANVAS_MEASURE, type CanvasWidth } from "./canvas/useEditorMode";
import { useAiAvailable } from "./useAiAvailable";
import { usePostLock } from "./usePostLock";
import { MOBILE_TOOLBAR_SLOT } from "./canvas/slot";
import type { CanvasHandle } from "./canvas/CanvasEditor";

// TipTap/ProseMirror is ~150 kB gzipped. The canvas is split out and fetched
// only when it is actually shown.
const CanvasEditor = React.lazy(() =>
  import("./canvas/CanvasEditor").then((m) => ({ default: m.CanvasEditor })),
);
import { PostChromeSidebar, type ChromeValue } from "./PostChromeSidebar";
import { FieldsPanel } from "./FieldsPanel";
import { AssistPanel, type LinkSuggestion } from "./AssistPanel";
import { SerpPreview } from "./SerpPreview";
import { SeoPanel } from "./SeoPanel";
import { appendFurtherReading, linkPhrase } from "./linking";
import { RevisionCompare, type RevisionSummary } from "./RevisionCompare";
import {
  countWords,
  normalizeBlocks,
  outlineOf,
  readingMinutes,
  type Block,
} from "./blocks";

/**
 * `new` starts a post that does not exist yet; the editor creates the draft
 * on the server after the first meaningful edit and swaps to `edit` in
 * place, so a closed tab never loses more than a couple of seconds' typing.
 */
type Mode = { kind: "new"; type?: string } | { kind: "edit"; id: string };

type SaveState = "clean" | "unsaved" | "saving" | "saved" | "error";

/** Statuses whose content readers can see; edits to these need "Update". */
const LIVE_STATUSES = new Set(["published", "scheduled", "private"]);

const AUTOSAVE_DELAY_MS = 2000;
const DRAFT_DELAY_MS = 1500;
const RETRY_DELAY_MS = 15000;
const MAX_AUTOSAVE_FAILURES = 5;

/** Images with a file but no alt text, anywhere in the document. */
export function imagesWithoutAlt(blocks: Block[]): number {
  let n = 0;
  const visit = (list: Block[]) => {
    for (const b of list ?? []) {
      if ((b.kind === "image" || b.kind === "media_text" || b.kind === "cover") && typeof b.attrs["url"] === "string" && b.attrs["url"] !== "") {
        const alt = b.attrs["alt"];
        if (typeof alt !== "string" || alt.trim() === "") n += 1;
      }
      visit(b.children ?? []);
    }
  };
  visit(blocks);
  return n;
}

/** True when a keystroke belongs to a field that manages its own undo. */
export function targetOwnsUndo(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.closest(".ProseMirror, input, textarea, [contenteditable=true]") !== null
  );
}

const EMPTY_CHROME: ChromeValue = {
  slug: "",
  excerpt: "",
  seo_title: "",
  seo_description: "",
  status: "draft",
  type: "post",
  password: undefined,
  sticky: false,
  lang: "",
  translation_of: "",
  translations: [],
  scheduled_for: "",
  parent_id: "",
  term_ids: [],
  featured_media_id: "",
  featured_media_url: "",
  featured_blurhash: "",
  featured_focal: "",
  seo_keyphrase: "",
  seo_canonical: "",
  seo_noindex: false,
  seo_nofollow: false,
  og_title: "",
  og_description: "",
  og_image: "",
  schema_type: "",
  product_price: "",
  product_currency: "",
};

/**
 * What a reader-facing post is missing. Nothing here blocks publishing;
 * the list is shown once so the author decides with it in view.
 */
export function publishChecklist(input: {
  title: string;
  blocks: Block[];
  excerpt: string;
  slug: string;
  termCount: number;
  isPage: boolean;
}): string[] {
  const out: string[] = [];
  if (input.title.trim() === "" || input.title.trim() === "Untitled") out.push("No title yet.");
  if (input.title.trim().length > 70) out.push("The title is over 70 characters; search results will cut it.");
  if (countWords(input.blocks) === 0) out.push("The post has no text.");
  const alt = imagesWithoutAlt(input.blocks);
  if (alt > 0) out.push(alt === 1 ? "1 image has no alt text." : `${alt} images have no alt text.`);
  if (input.excerpt.trim() === "") out.push("No excerpt; listings will use the opening lines.");
  if (/^untitled(-\d+)?$/.test(input.slug)) out.push(`The address is "/${input.slug}".`);
  if (!input.isPage && input.termCount === 0) out.push("No category or tag.");
  return out;
}

const SHORTCUTS: [string, string][] = [
  ["Ctrl+S", "Save"],
  ["Ctrl+K", "Add or edit a link"],
  ["Ctrl+B / I / U", "Bold, italic, underline"],
  ["Ctrl+E", "Inline code"],
  ["Ctrl+Shift+H", "Highlight"],
  ["Ctrl+Shift+S", "Strikethrough"],
  ["Ctrl+Z / Ctrl+Shift+Z", "Undo, redo"],
  ["Ctrl+Shift+F", "Focus mode"],
  ["/", "Insert a block (canvas)"],
  ["# ## ### at line start", "Heading"],
  ["- or 1. at line start", "List"],
  ["> at line start", "Quote"],
  ["```", "Code block"],
  ["?", "This sheet"],
];

/** The section tree as the server canonicalised it, or none. */
function asSections(value: unknown): Section[] {
  return Array.isArray(value) ? (value as Section[]) : [];
}

/** Segmented control entry for choosing the editing surface. */
function ModeButton({
  active,
  label,
  hint,
  onClick,
  children,
}: {
  active: boolean;
  label: string;
  hint: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={active}
      aria-label={label}
      title={`${label} — ${hint}`}
      onClick={onClick}
      className={cn(
        "inline-flex h-7 items-center gap-1.5 rounded-md px-2 text-xs transition-colors",
        active
          ? "bg-background font-medium text-foreground shadow-xs"
          : "text-muted-foreground hover:text-foreground",
      )}
    >
      {children}
      {label}
    </button>
  );
}

function SaveStatus({ state, savedAt }: { state: SaveState; savedAt: Date | null }) {
  switch (state) {
    case "saving":
      return (
        <span className="inline-flex items-center gap-1" aria-live="polite">
          <Cloud className="h-3 w-3 animate-pulse" aria-hidden="true" />
          Saving…
        </span>
      );
    case "unsaved":
      return (
        <span className="inline-flex items-center gap-1 text-amber-600 dark:text-amber-400" aria-live="polite">
          <span className="h-1.5 w-1.5 rounded-full bg-current" aria-hidden="true" />
          Unsaved changes
        </span>
      );
    case "error":
      return (
        <span className="inline-flex items-center gap-1 text-destructive" role="status">
          <CloudOff className="h-3 w-3" aria-hidden="true" />
          Couldn't save — retrying
        </span>
      );
    case "saved":
      return (
        <span className="inline-flex items-center gap-1" aria-live="polite">
          <Cloud className="h-3 w-3" aria-hidden="true" />
          Saved {savedAt !== null ? formatRelative(savedAt.toISOString()) : ""}
        </span>
      );
    default:
      return null;
  }
}

export function PostEditor({ mode }: { mode: Mode }) {
  const { can } = useCapabilities();
  const me = useMe();
  const navigate = useNavigate();
  const confirm = useConfirm();
  const routeId = mode.kind === "edit" ? mode.id : undefined;

  // The post this editor is working on. Starts undefined for a new post and
  // is filled in once the draft exists on the server.
  const [postId, setPostId] = React.useState<string | undefined>(routeId);
  const createdRef = React.useRef<string | null>(null);
  const hydratedFor = React.useRef<string | null>(null);
  const isNew = postId === undefined;

  const [title, setTitle] = React.useState("");
  const [blocks, setBlocks] = React.useState<Block[]>([]);
  const [chrome, setChrome] = React.useState<ChromeValue>({
    ...EMPTY_CHROME,
    type: mode.kind === "new" ? (mode.type ?? "post") : "post",
  });
  // A page's own section tree. Empty means "render through the theme's page
  // template", which is what every post and nearly every page does.
  const [layout, setLayout] = React.useState<Section[]>([]);
  // The type's custom field values, typed as the server takes them. They
  // ride with the prose: autosave, working copies and revisions carry them.
  const [fieldValues, setFieldValues] = React.useState<FieldValues>({});
  // What the entry itself holds (not a working copy): a save sends
  // `fields` only when they differ, because sending them replaces the
  // whole set — values a deleted field left behind included.
  const [savedFields, setSavedFields] = React.useState(() => fieldsKey({}));
  const [fieldErrors, setFieldErrors] = React.useState<Record<string, string>>({});
  const [fieldsMissing, setFieldsMissing] = React.useState<string[]>([]);
  const [showSections, setShowSections] = React.useState(false);
  const [showJson, setShowJson] = React.useState(false);
  const [jsonError, setJsonError] = React.useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = React.useState(false);
  const [showAssist, setShowAssist] = React.useState(false);
  const [showSerp, setShowSerp] = React.useState(false);
  const [showSeo, setShowSeo] = React.useState(false);
  const [showShortcuts, setShowShortcuts] = React.useState(false);
  // Set when the author chose to overwrite after a save conflict.
  const overwriteRef = React.useRef(false);
  const [focus, setFocus] = React.useState(false);
  const [savedAt, setSavedAt] = React.useState<Date | null>(null);
  const [postUpdatedAt, setPostUpdatedAt] = React.useState<string | null>(null);
  // What the server holds: its status, and a snapshot of the content readers
  // can see. A published post's edits live as a working copy until "Update".
  const [liveStatus, setLiveStatus] = React.useState("draft");
  const [liveContent, setLiveContent] = React.useState<string | null>(null);
  const [workingCopyAt, setWorkingCopyAt] = React.useState<string | null>(null);
  const [draftOffer, setDraftOffer] = React.useState<RevisionSummary | null>(null);
  const [compare, setCompare] = React.useState<RevisionSummary | null>(null);
  const [creating, setCreating] = React.useState(false);
  const [editorMode, setEditorMode] = useEditorMode();
  const [canvasWidth, setCanvasWidth] = useCanvasWidth();
  const ai = useAiAvailable();
  // The tag assist is told which tags already exist so it prefers them;
  // it used to be handed words from the draft instead, which made every
  // word the draft mentioned look like an existing tag.
  const tagNames = useQuery({
    queryKey: ["terms", "tag-names"],
    queryFn: () => api.listTerms({ taxonomy: "tag" }).then((ts) => ts.map((t) => t.name)),
    staleTime: 60_000,
    enabled: showAssist,
  });
  const canvasRef = React.useRef<CanvasHandle>(null);
  const titleRef = React.useRef<HTMLTextAreaElement>(null);
  // Below the desktop breakpoint the settings panel is a slide-over rather
  // than a second column, and the action bar lives at the bottom.
  const isDesktop = useMediaQuery("(min-width: 1024px)");

  // What the server last confirmed. Content is what autosave covers; the
  // rest of the settings only leave the browser on an explicit save.
  const isPage = chrome.type === "page";
  const contentSnapshot = React.useMemo(
    () => snapshotOf(title, blocks, chrome.excerpt, fieldValues),
    [title, blocks, chrome.excerpt, fieldValues],
  );
  const chromeSnapshot = React.useMemo(() => {
    const { excerpt: _excerpt, ...rest } = chrome;
    // The arrangement rides with the settings rather than the prose: it is
    // saved by an explicit save, never by autosave.
    return JSON.stringify({ ...rest, layout });
  }, [chrome, layout]);
  // A working copy holds a title and blocks, nothing else, so on a live
  // post an excerpt edit is a settings change: it stays "unsaved" until
  // Update carries it to the post, rather than reading as saved by a Save
  // that never sent it.
  const liveExcerptSnapshot = React.useMemo(
    () => JSON.stringify({ excerpt: chrome.excerpt }),
    [chrome.excerpt],
  );
  const [savedLiveExcerpt, setSavedLiveExcerpt] = React.useState<string | null>(null);
  const [savedContent, setSavedContent] = React.useState<string | null>(null);
  const [savedChrome, setSavedChrome] = React.useState<string | null>(null);

  const isLive = !isNew && LIVE_STATUSES.has(liveStatus);
  const contentDirty = isNew
    ? title.trim() !== "" || blocks.length > 0 || Object.keys(fieldValues).length > 0
    : savedContent !== null && contentSnapshot !== savedContent;
  const excerptDirty =
    isLive && savedLiveExcerpt !== null && liveExcerptSnapshot !== savedLiveExcerpt;
  const chromeDirty =
    (!isNew && savedChrome !== null && chromeSnapshot !== savedChrome) || excerptDirty;
  const dirty = contentDirty || chromeDirty;
  const unpublished = isLive && liveContent !== null && contentSnapshot !== liveContent;
  // The status the primary button applies: for a draft, publish (or
  // schedule when a date is set); otherwise whatever the sidebar says.
  const target =
    !isLive && (chrome.status === "" || chrome.status === "draft")
      ? chrome.scheduled_for !== ""
        ? "scheduled"
        : "published"
      : chrome.status || liveStatus;
  const primaryLabel = isLive
    ? target === "draft"
      ? "Unpublish"
      : "Update"
    : target === "scheduled"
      ? "Schedule"
      : target === "private"
        ? "Publish privately"
        : "Publish";
  const saveLabel = isLive ? "Save" : "Save draft";
  const canPublish = isNew || !isLive || unpublished || chromeDirty || target !== liveStatus;
  const dirtyRef = React.useRef(dirty);
  dirtyRef.current = dirty;
  const contentDirtyRef = React.useRef(contentDirty);
  contentDirtyRef.current = contentDirty;
  // Consecutive failed autosaves, and why autosave stopped if it did.
  const autosaveFailures = React.useRef(0);
  const [autosaveStopped, setAutosaveStopped] = React.useState<
    { reason: "failures" | "auth"; at: string } | null
  >(null);
  const resetAutosaveFailures = () => {
    autosaveFailures.current = 0;
    setAutosaveStopped(null);
  };
  const chromeRef = React.useRef(chrome);
  chromeRef.current = chrome;
  const bypassBlockRef = React.useRef(false);

  const historyRef = React.useRef<Block[][]>([]);
  const futureRef = React.useRef<Block[][]>([]);

  // The classic editor has no undo of its own, so the post editor keeps a
  // block-level history for it. The canvas brings TipTap's history and
  // hands us finished states only.
  const pushHistory = React.useCallback(
    (next: Block[]) => {
      historyRef.current.push(blocks);
      if (historyRef.current.length > 50) historyRef.current.shift();
      futureRef.current = [];
      setBlocks(next);
    },
    [blocks],
  );

  // Route changed to a different post than the one we are on (and not the
  // draft we just created): start over for it.
  React.useEffect(() => {
    if (routeId === postId) return;
    if (routeId !== undefined && routeId === createdRef.current) {
      setPostId(routeId);
      return;
    }
    setPostId(routeId);
    hydratedFor.current = null;
    setTitle("");
    setBlocks([]);
    setChrome({ ...EMPTY_CHROME, type: mode.kind === "new" ? (mode.type ?? "post") : "post" });
    setFieldValues({});
    setSavedFields(fieldsKey({}));
    setFieldErrors({});
    setFieldsMissing([]);
    setLayout([]);
    setShowSections(false);
    setSavedContent(null);
    setSavedChrome(null);
    setSavedAt(null);
    setDraftOffer(null);
    setLiveStatus("draft");
    setLiveContent(null);
    setWorkingCopyAt(null);
    setSavedLiveExcerpt(null);
    termsHydratedFor.current = null;
    offeredFor.current = null;
    historyRef.current = [];
    futureRef.current = [];
  }, [routeId]);

  const siteOptions = useQuery({ queryKey: ["options"], queryFn: () => api.getOptions() });
  const postQuery = useQuery({
    queryKey: ["post", postId],
    queryFn: () => api.getPost(postId as string),
    enabled: !isNew,
  });
  // The type is the server's for an existing entry: the editor's own
  // starts as "post" until the entry has loaded.
  const entryType = isNew ? chrome.type : String((postQuery.data as { type?: string } | undefined)?.type ?? "");
  const fieldDefs = useQuery({
    queryKey: ["content-fields", entryType],
    queryFn: () => api.listFields(entryType),
    enabled: entryType !== "" && entryType !== "block",
    staleTime: 60_000,
    retry: false,
  });
  const defs = fieldDefs.data ?? [];
  const hasFields = defs.length > 0;
  /**
   * Only keys the type defines: the server refuses any other (a value a
   * deleted field left, carried in by an old working copy) and keeps the
   * stored ones itself.
   */
  const definedOnly = (f: FieldValues): FieldValues => {
    const keys = new Set(defs.map((d) => d.key));
    return Object.fromEntries(Object.entries(f).filter(([k]) => keys.has(k)));
  };
  /**
   * Values as they compare: only defined keys once the definitions are
   * known, so a value a deleted field left in a revision is no difference.
   */
  const comparable = (f: FieldValues | null | undefined): string =>
    fieldsKey(fieldDefs.isSuccess ? definedOnly(f ?? {}) : f);
  const setField = (key: string, value: unknown) => {
    setFieldValues((f) => {
      const next = { ...f };
      if (value === undefined) delete next[key];
      else next[key] = value;
      return next;
    });
    setFieldErrors((e) => {
      if (e[key] === undefined) return e;
      const { [key]: _gone, ...rest } = e;
      return rest;
    });
  };
  /**
   * A revision's values as far as the type still defines them: the ones it
   * no longer has a field for are left out and named, rather than sent to
   * be refused.
   */
  const applyRevisionFields = (values: FieldValues | null | undefined) => {
    if (values === null || values === undefined) return;
    // Without the definitions there is nothing to judge by: take the
    // values as they are (a save sends only defined keys anyway).
    if (!fieldDefs.isSuccess) {
      setFieldValues({ ...values });
      setFieldErrors({});
      return;
    }
    const defined = new Set(defs.map((d) => d.key));
    const kept: FieldValues = {};
    const dropped: string[] = [];
    for (const [k, v] of Object.entries(values)) {
      if (defined.has(k)) kept[k] = v;
      else dropped.push(k);
    }
    setFieldValues(kept);
    setFieldErrors({});
    if (dropped.length > 0) {
      notify.info(
        "Some field values were not loaded",
        `This type has no field ${dropped.join(", ")} any more, so those values stay in the revision only.`,
      );
    }
  };
  const customTypes = useQuery({
    queryKey: ["content-types"],
    queryFn: () => api.listContentTypes(),
    staleTime: 60_000,
    enabled: chrome.type !== "post" && chrome.type !== "page",
  });
  const typeName = customTypes.data?.find((t) => t.slug === chrome.type)?.singular.toLowerCase();
  const canEdit = can("edit_posts") && (isNew || createdRef.current === postId || can("edit_others") || (postQuery.data !== undefined && String(postQuery.data.author_id) === String(me.data?.id)));
  const lock = usePostLock(canEdit ? postId : undefined);
  const publicPath = postPath(chrome.type, chrome.slug || "…", postQuery.data?.published_at ?? postQuery.data?.created_at ?? new Date().toISOString(), typeof siteOptions.data?.permalink_pattern === "string" ? siteOptions.data.permalink_pattern : undefined);


  const revsQuery = useQuery({
    queryKey: ["revisions", postId],
    queryFn: () => api.listRevisions(postId as string),
    enabled: !isNew && canEdit,
  });

  // The post's terms live on their own endpoint. Until they have loaded
  // the editor does not know them, and an update must not send an empty
  // list in their place — that stripped every category and tag from a post
  // on each Update.
  const termsQuery = useQuery({
    queryKey: ["post-terms", postId],
    queryFn: () => api.postTerms(postId as string),
    enabled: !isNew && canEdit,
  });
  const termsHydratedFor = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (postId === undefined || !termsQuery.data) return;
    if (termsHydratedFor.current === postId) return;
    termsHydratedFor.current = postId;
    const ids = termsQuery.data.map((t) => t.id);
    setChrome((c) => ({ ...c, term_ids: ids }));
    setSavedChrome((s) => {
      if (s === null) return s;
      const parsed = JSON.parse(s) as Record<string, unknown>;
      return JSON.stringify({ ...parsed, term_ids: ids });
    });
  }, [postId, termsQuery.data]);
  const termsKnown = isNew || termsHydratedFor.current === postId;

  /**
   * Turns assist-suggested tag names into term ids on the post.
   *
   * Names are matched case-insensitively against existing tags so a
   * suggestion of "Rust" does not create a duplicate of "rust"; anything
   * genuinely new is created. Existing tags on the post are kept.
   */
  const applyTags = async (names: string[]) => {
    if (names.length === 0) return;
    try {
      const existing = await api.listTerms({ taxonomy: "tag" });
      const byName = new Map(existing.map((t) => [t.name.toLowerCase(), t.id]));
      const ids: string[] = [];
      for (const name of names) {
        const found = byName.get(name.toLowerCase());
        if (found !== undefined) {
          ids.push(found);
          continue;
        }
        const created = await api.createTerm({ taxonomy: "tag", name });
        byName.set(name.toLowerCase(), created.id);
        ids.push(created.id);
      }
      setChrome((c) => ({
        ...c,
        term_ids: Array.from(new Set([...c.term_ids, ...ids])),
      }));
      notify.success(
        `Applied ${ids.length} tag${ids.length === 1 ? "" : "s"}`,
        "Save the post to keep them.",
      );
    } catch (e) {
      notify.error("Couldn't apply the tags", e);
    }
  };

  // Hydrate from the server once per post. A draft we created ourselves is
  // already hydrated — re-seeding from the server would drop keystrokes
  // typed while the create request was in flight.
  React.useEffect(() => {
    if (!postQuery.data || postId === undefined) return;
    if (hydratedFor.current === postId) return;
    hydratedFor.current = postId;
    const p = postQuery.data as unknown as {
      title: string;
      content: unknown;
      excerpt: string | null;
      slug: string;
      status: string;
      type: string;
      updated_at: string;
      layout: unknown;
      scheduled_for: string | null;
      parent_id: string | number | null;
    };
    const nextBlocks = normalizeBlocks((p.content as { blocks?: unknown })?.blocks);
    const meta = (p as unknown as { meta?: Record<string, unknown> }).meta ?? {};
    const str = (v: unknown) => (typeof v === "string" ? v : "");
    const nextChrome: ChromeValue = {
      ...EMPTY_CHROME,
      slug: p.slug ?? "",
      excerpt: p.excerpt ?? "",
      seo_title: typeof meta["seo_title"] === "string" ? meta["seo_title"] : "",
      seo_description: typeof meta["seo_description"] === "string" ? meta["seo_description"] : "",
      status: p.status ?? "draft",
      type: p.type ?? "post",
      scheduled_for: p.scheduled_for ?? "",
      parent_id: p.parent_id === null || p.parent_id === undefined ? "" : String(p.parent_id),
      featured_media_id: typeof meta["featured_media_id"] === "string" || typeof meta["featured_media_id"] === "number" ? String(meta["featured_media_id"]) : "",
      featured_media_url: typeof meta["featured_media_url"] === "string" ? meta["featured_media_url"] : "",
      featured_blurhash: typeof meta["featured_blurhash"] === "string" ? meta["featured_blurhash"] : "",
      featured_focal: str(meta["featured_focal"]),
      seo_keyphrase: str(meta["seo_keyphrase"]),
      seo_canonical: str(meta["seo_canonical"]),
      seo_noindex: meta["seo_noindex"] === true,
      seo_nofollow: meta["seo_nofollow"] === true,
      og_title: str(meta["og_title"]),
      og_description: str(meta["og_description"]),
      og_image: str(meta["og_image"]),
      schema_type: str(meta["schema_type"]),
      product_price: str(meta["product_price"]),
      product_currency: str(meta["product_currency"]),
      sticky: Boolean((p as unknown as { sticky?: boolean }).sticky),
      lang: String((p as unknown as { lang?: string }).lang ?? ""),
      translation_of: "",
      translations: ((p as unknown as { translations?: { id: string; lang: string; title: string; status: string }[] }).translations ?? []),
    };
    const nextFields = fieldsOf(p);
    setTitle(p.title ?? "");
    setBlocks(nextBlocks);
    setFieldValues(nextFields);
    setSavedFields(fieldsKey(nextFields));
    setFieldErrors({});
    setFieldsMissing(postQuery.data.fields_missing ?? []);
    setLayout(Array.isArray(p.layout) ? (p.layout as Section[]) : []);
    setChrome((c) => ({ ...nextChrome, term_ids: c.term_ids, password: c.password }));
    setPostUpdatedAt(p.updated_at ?? null);
    const snapshot = snapshotOf(p.title ?? "", nextBlocks, p.excerpt ?? "", nextFields);
    setSavedContent(snapshot);
    setLiveStatus(p.status ?? "draft");
    setLiveContent(snapshot);
    setWorkingCopyAt(null);
    setSavedLiveExcerpt(JSON.stringify({ excerpt: p.excerpt ?? "" }));
    const { excerpt: _e, ...rest } = nextChrome;
    // Terms arrive from their own query and are folded in when they do.
    setSavedChrome(
      JSON.stringify({ ...rest, term_ids: chromeRef.current.term_ids, layout: asSections(p.layout) }),
    );
  }, [postQuery.data, postId]);

  // A revision newer than the post itself is work that was never published.
  // A saved working copy is loaded straight into the editor — the author
  // chose to keep it. An autosave is only offered, once per post: it may be
  // a session that ended mid-thought.
  const offeredFor = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (postId === undefined || !postQuery.data || !revsQuery.data) return;
    if (offeredFor.current === postId || postUpdatedAt === null) return;
    // Decided once, so only once the type's fields are known (or known to
    // be unavailable): before that, an old key in a revision would look
    // like a change.
    if (fieldDefs.isPending && fieldDefs.fetchStatus !== "idle") return;
    offeredFor.current = postId;
    const norm = (f: FieldValues): FieldValues => (fieldDefs.isSuccess ? definedOnly(f) : f);
    const post = postQuery.data as unknown as {
      title: string;
      content: unknown;
      excerpt: string | null;
    };
    const postBlocks = normalizeBlocks((post.content as { blocks?: unknown })?.blocks);
    const sorted = [...revsQuery.data].sort(
      (a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime(),
    );
    const at = (iso: string) => new Date(iso).getTime();
    const blocksOf = (r: RevisionSummary) =>
      normalizeBlocks((r.content as { blocks?: unknown } | null)?.blocks);
    // A revision from before fields existed carries none; it differs only
    // by its text.
    const same = (r: RevisionSummary, t: string, b: Block[], f: FieldValues) =>
      r.title === t &&
      JSON.stringify(blocksOf(r)) === JSON.stringify(b) &&
      (r.fields === null || r.fields === undefined || comparable(r.fields) === comparable(f));

    // What the editor holds after this: the post, or a saved working copy.
    let baseTitle = post.title;
    let baseBlocks = postBlocks;
    let baseFields = fieldsOf(post);
    let baseAt = at(postUpdatedAt);
    const manual = sorted.find((r) => !r.is_autosave);
    if (manual !== undefined && at(manual.created_at) > baseAt && !same(manual, baseTitle, baseBlocks, baseFields)) {
      baseTitle = manual.title;
      baseBlocks = blocksOf(manual);
      if (manual.fields !== null && manual.fields !== undefined) baseFields = norm(manual.fields);
      baseAt = at(manual.created_at);
      setTitle(baseTitle);
      setBlocks(baseBlocks);
      setFieldValues(baseFields);
      setSavedContent(snapshotOf(baseTitle, baseBlocks, post.excerpt ?? "", baseFields));
      setWorkingCopyAt(manual.created_at);
    }
    // An autosave newer than that is a session that ended mid-thought.
    const auto = sorted.find((r) => r.is_autosave);
    if (auto !== undefined && at(auto.created_at) > baseAt && !same(auto, baseTitle, baseBlocks, baseFields)) {
      setDraftOffer(auto);
    }
  }, [postId, postQuery.data, revsQuery.data, postUpdatedAt, fieldDefs.isPending, fieldDefs.fetchStatus, fieldDefs.isSuccess]);

  const docForSave = React.useMemo(
    () => ({ schema_version: 1 as const, blocks }),
    [blocks],
  );

  // The post's metadata with the sidebar's SEO fields folded in; other
  // keys (featured image, plugin data) are preserved.
  const metaForSave = (c: ChromeValue = chrome) => {
    const existing =
      ((postQuery.data as unknown as { meta?: Record<string, unknown> } | undefined)?.meta) ?? {};
    const next: Record<string, unknown> = {
      ...existing,
      seo_title: c.seo_title,
      seo_description: c.seo_description,
    };
    // SEO settings ride in meta; an empty one is removed rather than
    // stored as "", so the head falls back to the entry itself.
    const seoText: [string, string][] = [
      ["seo_keyphrase", c.seo_keyphrase],
      ["seo_canonical", c.seo_canonical],
      ["og_title", c.og_title],
      ["og_description", c.og_description],
      ["og_image", c.og_image],
      ["schema_type", c.schema_type],
      ["product_price", c.product_price],
      ["product_currency", c.product_currency],
    ];
    for (const [key, value] of seoText) {
      if (value.trim() === "") delete next[key];
      else next[key] = value.trim();
    }
    for (const [key, on] of [["seo_noindex", c.seo_noindex], ["seo_nofollow", c.seo_nofollow]] as const) {
      if (on) next[key] = true;
      else delete next[key];
    }
    // The featured image lives in meta under the keys the theme's cards
    // and the page's social tags already read.
    if (c.featured_media_url !== "") {
      next["featured_media_id"] = c.featured_media_id;
      next["featured_media_url"] = c.featured_media_url;
      next["featured_blurhash"] = c.featured_blurhash;
      if (c.featured_focal !== "") next["featured_focal"] = c.featured_focal;
      else delete next["featured_focal"];
    } else {
      delete next["featured_media_id"];
      delete next["featured_media_url"];
      delete next["featured_blurhash"];
      delete next["featured_focal"];
    }
    return next;
  };

  /**
   * Everything one save sends and, on success, marks as saved — taken at
   * the moment the save starts. TanStack v5 hands a pending mutation the
   * latest render's callbacks, so reading `contentSnapshot` in `onSuccess`
   * would mark text typed during the request as saved.
   */
  interface SaveVars {
    status: string;
    title: string;
    doc: { schema_version: 1; blocks: Block[] };
    chrome: ChromeValue;
    layout: Section[];
    content: string;
    liveExcerpt: string;
    fields: FieldValues;
  }
  const saveVars = (status = ""): SaveVars => ({
    status,
    title,
    doc: docForSave,
    chrome,
    layout,
    content: contentSnapshot,
    liveExcerpt: liveExcerptSnapshot,
    fields: fieldValues,
  });
  /** `fields` for a write to the entry, only when they differ from what it holds. */
  const fieldsForSave = (f: FieldValues): { fields?: FieldValues } => {
    if (!fieldDefs.isSuccess) return {};
    const sent = definedOnly(f);
    return fieldsKey(sent) === savedFields ? {} : { fields: sent };
  };
  /** `fields` for an autosave or working copy, once the type's fields are known. */
  const fieldsForCopy = (f: FieldValues): { fields?: FieldValues } =>
    hasFields ? { fields: definedOnly(f) } : {};
  /** Required fields with no value: publishing or scheduling would be refused. */
  const missingRequired = () =>
    defs.filter((d) => {
      if (!d.required) return false;
      const v = fieldValues[d.key];
      return v === undefined || v === null || v === "" || (Array.isArray(v) && v.length === 0);
    });
  /** After a write the server took: what the entry now holds, and what it found missing. */
  const fieldsSaved = (p: PostResponse, sent: SaveVars) => {
    // What the entry holds now, as the server answered; the values sent
    // when an answer carries none.
    setSavedFields(fieldsKey(p.fields !== null && typeof p.fields === "object" ? fieldsOf(p) : definedOnly(sent.fields)));
    setFieldErrors({});
    setFieldsMissing(p.fields_missing ?? []);
  };
  /** A refused write: each field's message goes next to the field. */
  const fieldsRefused = (e: unknown) => {
    const errs = fieldErrorsOf(e);
    if (Object.keys(errs).length > 0) setFieldErrors(errs);
  };
  const saveVarsRef = React.useRef(saveVars);
  saveVarsRef.current = saveVars;

  /** One question when the server says the post moved under us. */
  const handleConflict = async (e: unknown, retry: () => void): Promise<boolean> => {
    if (!(e instanceof ApiError) || e.status !== 409 || !e.message.includes("changed since")) return false;
    const overwrite = await confirm({
      title: "This post changed since you opened it",
      description:
        "Another tab or another person saved it. Overwrite with your version, or reload theirs — your current text is saved as an autosave first, and you can restore it from the banner or the revision history.",
      confirmLabel: "Overwrite",
      cancelLabel: "Reload theirs",
      destructive: true,
    });
    if (overwrite) {
      overwriteRef.current = true;
      retry();
    } else {
      // Keep the author's text before theirs replaces it. If that fails,
      // stay put: reloading would lose the only copy.
      if (contentDirtyRef.current && postId !== undefined) {
        try {
          await autosave.mutateAsync(saveVarsRef.current());
        } catch (err) {
          notify.error("Couldn't keep your edits, so nothing was reloaded", err);
          return true;
        }
      }
      await revsQuery.refetch();
      // Let the newer autosave be offered against their version.
      offeredFor.current = null;
      hydratedFor.current = null;
      await postQuery.refetch();
    }
    return true;
  };
  const expectedUpdatedAt = () => {
    if (overwriteRef.current) {
      overwriteRef.current = false;
      return undefined;
    }
    return postUpdatedAt ?? undefined;
  };

  const saveNew = useMutation({
    mutationFn: ({ status, title, doc, chrome, layout, fields }: SaveVars) =>
      api.createPost({
        title: title || "Untitled",
        content: doc,
        status,
        type: chrome.type || "post",
        slug: chrome.slug || null,
        excerpt: chrome.excerpt || null,
        password: chrome.password || null,
        sticky: chrome.sticky,
        lang: chrome.lang,
        ...(chrome.translation_of ? { translation_of: chrome.translation_of } : {}),
        term_ids: chrome.term_ids.length ? chrome.term_ids : null,
        scheduled_for: chrome.scheduled_for || null,
        parent_id: chrome.parent_id === "" ? null : chrome.parent_id,
        layout: chrome.type === "page" ? layout : undefined,
        ...(Object.keys(definedOnly(fields)).length > 0 ? { fields: definedOnly(fields) } : {}),
      }),
    onSuccess: (p, sent) => {
      fieldsSaved(p, sent);
      const id = String(p.id);
      createdRef.current = id;
      hydratedFor.current = id;
      setPostId(id);
      setSavedContent(sent.content);
      resetAutosaveFailures();
      const savedLayout = sent.chrome.type === "page" ? asSections(p.layout) : sent.layout;
      if (sent.chrome.type === "page") setLayout(savedLayout);
      const { excerpt: _e, ...restChrome } = sent.chrome;
      setSavedChrome(JSON.stringify({ ...restChrome, layout: savedLayout }));
      setSavedAt(new Date());
      setLiveStatus(p.status);
      setLiveContent(sent.content);
      setChrome((c) => ({ ...c, status: p.status }));
      notify.success(LIVE_STATUSES.has(p.status) ? "Post published" : "Draft saved");
      bypassBlockRef.current = true;
      void navigate({ to: "/posts/$postId", params: { postId: id }, replace: true });
    },
    onError: (e) => {
      fieldsRefused(e);
      notify.error("Couldn't create the post", e);
    },
  });

  // "Publish" / "Update" / "Unpublish": writes the post itself, so readers
  // see it. The only path that changes what is live.
  const updateLive = useMutation({
    mutationFn: ({ status, title, doc, chrome, layout, fields }: SaveVars) =>
      api.updatePost(postId as string, {
        title,
        content: doc,
        status,
        meta: metaForSave(chrome),
        ...fieldsForSave(fields),
        slug: chrome.slug || undefined,
        excerpt: chrome.excerpt,
        password: chrome.password,
        sticky: chrome.sticky,
        lang: chrome.lang,
        ...(chrome.translation_of ? { translation_of: chrome.translation_of } : {}),
        term_ids: termsKnown ? chrome.term_ids : undefined,
        scheduled_for: chrome.scheduled_for || undefined,
        // Sent only for pages, and always in full: an empty array is how an
        // author clears the composition and goes back to the theme template.
        layout: chrome.type === "page" ? layout : undefined,
        expected_updated_at: expectedUpdatedAt(),
      }),
    onSuccess: (p, sent) => {
      fieldsSaved(p, sent);
      const { status } = sent;
      const wasLive = isLive;
      const nowStatus = p.status ?? status;
      setSavedAt(new Date());
      setSavedContent(sent.content);
      resetAutosaveFailures();
      setChrome((c) => ({ ...c, status: nowStatus }));
      const sentPage = sent.chrome.type === "page";
      const savedLayout = sentPage ? asSections(p.layout) : sent.layout;
      if (sentPage) setLayout(savedLayout);
      const { excerpt: _e, ...restChrome } = { ...sent.chrome, status: nowStatus };
      setSavedChrome(JSON.stringify({ ...restChrome, layout: savedLayout }));
      setLiveStatus(nowStatus);
      setLiveContent(sent.content);
      setSavedLiveExcerpt(sent.liveExcerpt);
      setWorkingCopyAt(null);
      setPostUpdatedAt((p as unknown as { updated_at?: string }).updated_at ?? new Date().toISOString());
      setDraftOffer(null);
      void revsQuery.refetch();
      const nowLive = LIVE_STATUSES.has(p.status ?? status);
      notify.success(
        nowLive && !wasLive
          ? status === "scheduled"
            ? "Post scheduled"
            : "Post published"
          : nowLive
            ? "Changes published"
            : wasLive
              ? "Post unpublished"
              : "Draft saved",
      );
    },
    onError: (e, sent) => {
      fieldsRefused(e);
      // A retry after "Overwrite" sends what the editor holds by then.
      void handleConflict(e, () => updateLive.mutate(saveVarsRef.current(sent.status))).then((handled) => {
        if (!handled) notify.error("Couldn't update the post", e);
      });
    },
  });

  // "Save draft": a draft is not live, so saving it is just saving it. The
  // status is deliberately left alone — this button never publishes.
  const saveDraft = useMutation({
    mutationFn: ({ title, doc, chrome, layout, fields }: SaveVars) =>
      api.updatePost(postId as string, {
        title,
        content: doc,
        meta: metaForSave(chrome),
        ...fieldsForSave(fields),
        slug: chrome.slug || undefined,
        excerpt: chrome.excerpt,
        password: chrome.password,
        sticky: chrome.sticky,
        lang: chrome.lang,
        ...(chrome.translation_of ? { translation_of: chrome.translation_of } : {}),
        term_ids: termsKnown ? chrome.term_ids : undefined,
        scheduled_for: chrome.scheduled_for || undefined,
        layout: chrome.type === "page" ? layout : undefined,
        expected_updated_at: expectedUpdatedAt(),
      }),
    onSuccess: (p, sent) => {
      fieldsSaved(p, sent);
      setSavedAt(new Date());
      setSavedContent(sent.content);
      resetAutosaveFailures();
      const sentPage = sent.chrome.type === "page";
      const savedLayout = sentPage ? asSections(p.layout) : sent.layout;
      if (sentPage) setLayout(savedLayout);
      const { excerpt: _e, ...restChrome } = sent.chrome;
      setSavedChrome(JSON.stringify({ ...restChrome, layout: savedLayout }));
      setLiveContent(sent.content);
      setSavedLiveExcerpt(sent.liveExcerpt);
      setPostUpdatedAt((p as unknown as { updated_at?: string }).updated_at ?? new Date().toISOString());
      setDraftOffer(null);
      void revsQuery.refetch();
      notify.success("Draft saved");
    },
    onError: (e) => {
      fieldsRefused(e);
      void handleConflict(e, () => saveDraft.mutate(saveVarsRef.current())).then((handled) => {
        if (!handled) notify.error("Couldn't save the draft", e);
      });
    },
  });

  // "Save" on a live post: a working copy on the server, nothing published.
  const saveWorking = useMutation({
    mutationFn: ({ title, doc, fields }: SaveVars) =>
      api.saveRevision(postId as string, { title, content: doc, ...fieldsForCopy(fields) }),
    onSuccess: (rev, sent) => {
      setFieldErrors({});
      setSavedAt(new Date());
      setSavedContent(sent.content);
      resetAutosaveFailures();
      setWorkingCopyAt(rev.created_at);
      void revsQuery.refetch();
      notify.success("Saved — not published yet", "Update when you want it live.");
    },
    onError: (e) => {
      fieldsRefused(e);
      notify.error("Couldn't save your changes", e);
    },
  });

  const autosave = useMutation({
    mutationFn: ({ title, doc, chrome, fields }: SaveVars) =>
      api.autosave(postId as string, {
        title,
        content: doc,
        excerpt: chrome.excerpt || null,
        ...fieldsForCopy(fields),
      }),
    onSuccess: (_r, sent) => {
      setSavedAt(new Date());
      setSavedContent(sent.content);
      resetAutosaveFailures();
    },
    onError: (e, sent) => {
      // TanStack v5 counts failures per mutate call, so the cap is kept
      // here: consecutive failed autosaves across calls.
      autosaveFailures.current += 1;
      const refused = fieldErrorsOf(e);
      if (e instanceof ApiError && e.status === 401) {
        // Signed out: retrying cannot succeed until the author signs in.
        setAutosaveStopped({ reason: "auth", at: sent.content });
      } else if (Object.keys(refused).length > 0) {
        // A value the server refuses will be refused again: say which,
        // and wait for the next edit rather than retrying.
        setFieldErrors(refused);
        setAutosaveStopped({ reason: "failures", at: sent.content });
      } else if (autosaveFailures.current >= MAX_AUTOSAVE_FAILURES) {
        setAutosaveStopped({ reason: "failures", at: sent.content });
      }
    },
  });

  const doTrash = useMutation({
    mutationFn: () => api.trashPost(postId as string),
    onSuccess: () => {
      notify.success("Moved to trash");
      bypassBlockRef.current = true;
      const type = chromeRef.current.type;
      if (type === "post" || type === "page") void navigate({ to: "/posts", search: { page: 1 } });
      else void navigate({ to: "/entries/$type", params: { type } });
    },
    onError: (e) => notify.error("Couldn't move to trash", e),
  });

  // Loading a revision is an edit like any other: it goes into the editor,
  // and Save or Update decides what happens to it. Restoring straight to the
  // server would publish it on a live post.
  const loadRevision = (r: RevisionSummary) => {
    setTitle(r.title);
    setBlocks(normalizeBlocks((r.content as { blocks?: unknown } | null)?.blocks));
    applyRevisionFields(r.fields);
    setCompare(null);
    setDraftOffer(null);
    notify.success("Revision loaded", isLive ? "Update to publish it, or Save to keep it." : "Save to keep it.");
  };

  // Throw away the working copy: back to what readers see. A revision equal
  // to the live content is recorded so the next visit doesn't load the old
  // working copy again.
  const discardWorkingCopy = () => {
    if (liveContent === null) return;
    const live = JSON.parse(liveContent) as { title: string; blocks: Block[]; excerpt: string; fields?: string };
    // The snapshot holds the values as ordered pairs (see `fieldsKey`).
    const liveFields: FieldValues = Object.fromEntries(
      (JSON.parse(live.fields ?? "[]") as [string, unknown][]),
    );
    setTitle(live.title);
    setBlocks(live.blocks);
    setFieldValues(liveFields);
    setFieldErrors({});
    setChrome((c) => ({ ...c, excerpt: live.excerpt }));
    setSavedContent(liveContent);
    setWorkingCopyAt(null);
    if (postId !== undefined) {
      api
        .saveRevision(postId, { title: live.title, content: { schema_version: 1, blocks: live.blocks }, ...fieldsForCopy(liveFields) })
        .then(() => revsQuery.refetch())
        .catch((e: unknown) => notify.error("Couldn't record the discard", e));
    }
  };

  // Debounced autosave once there is a post to save into.
  React.useEffect(() => {
    if (isNew || !contentDirty || !canEdit) return;
    // Stopped after repeated failures: a fresh edit tries again. Stopped
    // because the session ended: only a manual save (after signing in).
    if (autosaveStopped !== null) {
      if (autosaveStopped.reason === "auth" || autosaveStopped.at === contentSnapshot) return;
      autosaveFailures.current = 0;
      setAutosaveStopped(null);
    }
    const vars = saveVars();
    const t = setTimeout(() => autosave.mutate(vars), AUTOSAVE_DELAY_MS);
    return () => clearTimeout(t);
    // `autosave` is deliberately not a dependency: the mutation object is a
    // new identity on every render, which would restart the timer forever.
  }, [contentSnapshot, contentDirty, isNew, canEdit]);

  // A failed autosave retries on its own a few times before giving up until
  // the next edit — a flaky connection should not cost anyone their draft.
  React.useEffect(() => {
    if (!autosave.isError || autosaveStopped !== null) return;
    const t = setTimeout(() => {
      if (contentDirtyRef.current) autosave.mutate(saveVarsRef.current());
    }, RETRY_DELAY_MS);
    return () => clearTimeout(t);
  }, [autosave.isError, autosave.submittedAt, autosaveStopped]);

  // A new post becomes a server-side draft after the first meaningful edit.
  React.useEffect(() => {
    if (!isNew || creating || !contentDirty) return;
    // Field values wait for the first save or autosave: a value the server
    // refuses must not stop the draft from existing.
    const sent = snapshotOf(title, blocks, chrome.excerpt, {});
    const t = setTimeout(() => {
      setCreating(true);
      api
        .createPost({
          title: title || "Untitled",
          content: docForSave,
          status: "draft",
          type: chrome.type || "post",
        })
        .then((p) => {
          const id = String(p.id);
          createdRef.current = id;
          hydratedFor.current = id;
          setPostId(id);
          setSavedContent(sent);
          setSavedChrome(chromeSnapshot);
          setSavedAt(new Date());
          setPostUpdatedAt(new Date().toISOString());
          setLiveStatus("draft");
          setLiveContent(sent);
          offeredFor.current = id; // nothing older than us exists
          bypassBlockRef.current = true;
          void navigate({ to: "/posts/$postId", params: { postId: id }, replace: true });
        })
        .catch((e: unknown) => notify.error("Couldn't start a draft", e))
        .finally(() => setCreating(false));
    }, DRAFT_DELAY_MS);
    return () => clearTimeout(t);
  }, [contentSnapshot, contentDirty, isNew, creating]);

  // Leaving with unsaved work asks first — both in-app navigation and the
  // tab closing. Our own replace-navigation after creating a draft bypasses
  // the question.
  useBlocker({
    shouldBlockFn: async () => {
      if (bypassBlockRef.current) {
        bypassBlockRef.current = false;
        return false;
      }
      if (!dirtyRef.current) return false;
      const leave = await confirm({
        title: "Leave without saving?",
        description: "Changes since the last save will be lost.",
        confirmLabel: "Leave",
        destructive: true,
      });
      return !leave;
    },
    enableBeforeUnload: () => dirtyRef.current,
  });

  // Save never publishes; Publish/Update is the only thing that does.
  const save = React.useCallback(() => {
    if (!canEdit) return;
    const vars = saveVarsRef.current("draft");
    if (isNew) saveNew.mutate(vars);
    else if (isLive) saveWorking.mutate(vars);
    else saveDraft.mutate(vars);
  }, [canEdit, isNew, isLive, saveNew, saveWorking, saveDraft]);

  const publish = React.useCallback(() => {
    if (!canEdit || !can("publish_posts")) return;
    // The server refuses to publish or schedule without the required
    // fields; say which here rather than send a request bound to fail.
    if (target === "published" || target === "scheduled") {
      const missing = missingRequired();
      if (missing.length > 0) {
        setFieldErrors((e) => ({
          ...e,
          ...Object.fromEntries(missing.map((d) => [d.key, `${d.label} is required to publish or schedule`])),
        }));
        notify.error("Fill in the required fields to publish", missing.map((d) => d.label).join(", "));
        return;
      }
    }
    const go = () => {
      const vars = saveVarsRef.current(target);
      if (isNew) saveNew.mutate(vars);
      else updateLive.mutate(vars);
    };
    // Going live for the first time gets the checklist; an Update to a
    // post that is already live does not nag.
    if (isLive) {
      go();
      return;
    }
    const issues = publishChecklist({
      title,
      blocks,
      excerpt: chrome.excerpt,
      slug: chrome.slug,
      termCount: chrome.term_ids.length,
      // Categories and tags are a post's; pages and other types go without.
      isPage: chrome.type !== "post",
    });
    if (issues.length === 0) {
      go();
      return;
    }
    void confirm({
      title: "Before you publish",
      description: (
        <ul className="list-disc space-y-1 pl-4" data-testid="publish-checklist">
          {issues.map((i) => (
            <li key={i}>{i}</li>
          ))}
        </ul>
      ),
      confirmLabel: "Publish anyway",
      cancelLabel: "Keep editing",
    }).then((ok) => {
      if (ok) go();
    });
  }, [can, canEdit, isNew, isLive, target, saveNew, updateLive, title, blocks, chrome.excerpt, chrome.slug, chrome.term_ids.length, chrome.type, fieldValues, defs, confirm]);
  // Publish or Save while the background draft is still being created
  // would create a second post; the buttons wait for the first. So do
  // they while someone else holds the editing lock.
  const busy =
    saveNew.isPending || updateLive.isPending || saveDraft.isPending || saveWorking.isPending || creating || lock.heldByOther;

  // Ctrl/Cmd+S saves from anywhere in the editor. Ctrl/Cmd+Z steps the
  // block history — but only when the keystroke is not inside a field that
  // has its own undo (the canvas, the title, a textarea), which must not be
  // pre-empted. Ctrl/Cmd+Shift+F toggles focus mode; Escape leaves it.
  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && sidebarOpen && !isDesktop) {
        setSidebarOpen(false);
        return;
      }
      if (e.key === "Escape" && focus) {
        setFocus(false);
        return;
      }
      if (e.key === "?" && !e.ctrlKey && !e.metaKey && !e.altKey && !targetOwnsUndo(e.target)) {
        e.preventDefault();
        setShowShortcuts((v) => !v);
        return;
      }
      const mod = e.ctrlKey || e.metaKey;
      if (!mod) return;
      const key = e.key.toLowerCase();
      if (key === "s") {
        e.preventDefault();
        save();
        return;
      }
      if (key === "f" && e.shiftKey) {
        e.preventDefault();
        setFocus((v) => !v);
        return;
      }
      if (key === "z" && !targetOwnsUndo(e.target)) {
        e.preventDefault();
        if (e.shiftKey) {
          const nxt = futureRef.current.pop();
          if (nxt) {
            historyRef.current.push(blocks);
            setBlocks(nxt);
          }
        } else {
          const prev = historyRef.current.pop();
          if (prev) {
            futureRef.current.push(blocks);
            setBlocks(prev);
          }
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [blocks, save, focus, sidebarOpen, isDesktop]);

  // The title wraps instead of scrolling sideways; grow it to fit.
  React.useLayoutEffect(() => {
    const el = titleRef.current;
    if (el === null) return;
    el.style.height = "0px";
    el.style.height = `${el.scrollHeight}px`;
  }, [title, focus, sidebarOpen, isDesktop]);

  // The slide-over owns the screen while it is open on small screens.
  React.useEffect(() => {
    if (!sidebarOpen || isDesktop) return;
    const { overflow } = document.body.style;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = overflow;
    };
  }, [sidebarOpen, isDesktop]);

  // "Saved 2 minutes ago" only stays true if something re-renders it.
  const [, tick] = React.useReducer((n: number) => n + 1, 0);
  React.useEffect(() => {
    const t = setInterval(tick, 30_000);
    return () => clearInterval(t);
  }, []);

  const askTrash = async () => {
    const ok = await confirm({
      title: `Move “${title || "this post"}” to trash?`,
      description:
        "It stops appearing on your site. You can restore it from the posts list.",
      confirmLabel: "Move to trash",
      destructive: true,
    });
    if (ok) doTrash.mutate();
  };

  const openPreview = async () => {
    if (postId === undefined) return;
    try {
      const { url } = await api.previewToken(postId as string);
      window.open(api.previewPageUrl(url), "_blank");
    } catch (e) {
      notify.error("Couldn't open a preview", e);
    }
  };

  const focusBody = () => {
    if (editorMode === "canvas" && !showJson) {
      canvasRef.current?.focus();
      return;
    }
    document
      .querySelector<HTMLElement>(
        '[data-testid="block-editor"] textarea, [data-testid="block-editor"] input, [data-testid="json-editor"]',
      )
      ?.focus();
  };

  const jumpToHeading = (index: number) => {
    const headings = document.querySelectorAll<HTMLElement>(
      '.vy-canvas :is(h2,h3,h4,h5,h6), [data-vy-kind="heading"]',
    );
    const el = headings[index];
    if (el === undefined) return;
    el.scrollIntoView({ block: "center", behavior: "smooth" });
    el.focus?.();
  };

  const takeDraftOffer = () => {
    if (draftOffer === null) return;
    setTitle(draftOffer.title);
    setBlocks(normalizeBlocks((draftOffer.content as { blocks?: unknown } | null)?.blocks));
    applyRevisionFields(draftOffer.fields);
    setDraftOffer(null);
    notify.success("Draft restored", "Save when you're happy with it.");
  };

  const saving = busy;
  const saveState: SaveState =
    saving || autosave.isPending || creating
      ? "saving"
      : autosave.isError && contentDirty
        ? "error"
        : dirty
          ? "unsaved"
          : savedAt !== null
            ? "saved"
            : "clean";
  const words = countWords(blocks);
  const minutes = readingMinutes(words);
  const outline = outlineOf(blocks);
  const missingAlt = imagesWithoutAlt(blocks);
  const revisions = [...(revsQuery.data ?? [])].sort(
    (a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime(),
  );

  if (!isNew && postQuery.isSuccess && !canEdit) return <p role="alert">You do not have permission to edit this post.</p>;

  const surface = showSections ? (
    <PageSections
      sections={layout}
      onChange={setLayout}
      postId={postId}
      document={docForSave}
    />
  ) : showJson ? (
    <JsonPane
      document={docForSave}
      error={jsonError}
      onChange={(text) => {
        try {
          const j = JSON.parse(text) as { blocks?: unknown };
          setJsonError(null);
          pushHistory(normalizeBlocks(j.blocks));
        } catch (err) {
          setJsonError((err as Error).message);
        }
      }}
    />
  ) : editorMode === "canvas" ? (
    <React.Suspense
      fallback={<div className="min-h-[50vh] animate-pulse rounded-lg bg-muted/40" />}
    >
      <CanvasEditor ref={canvasRef} value={blocks} onChange={setBlocks} />
    </React.Suspense>
  ) : (
    <BlockEditor value={blocks} onChange={pushHistory} />
  );

  return (
    <div
      className={cn(
        // Fill the viewport below the shell header so the sticky action bar
        // sits at the bottom even for a short post: header (3.5rem) plus the
        // main padding at each breakpoint.
        "flex min-h-[calc(100dvh-5rem)] flex-col sm:min-h-[calc(100dvh-5.5rem)] md:min-h-[calc(100dvh-6.5rem)]",
        focus && "fixed inset-0 z-40 overflow-y-auto bg-background p-3 sm:p-4 md:p-6",
      )}
      data-testid="post-editor"
      data-focus-mode={focus ? "true" : undefined}
    >
      {focus ? (
        <div className="mx-auto mb-6 flex max-w-(--vy-canvas-measure,70ch) items-center gap-3 text-xs text-muted-foreground">
          <Button variant="outline" size="sm" onClick={() => setFocus(false)}>
            <Minimize2 className="h-3.5 w-3.5" aria-hidden="true" />
            Exit focus
            <kbd className="ml-1 rounded border px-1 font-mono text-[10px]">Esc</kbd>
          </Button>
          <span>{words === 1 ? "1 word" : `${words} words`}</span>
          <span>·</span>
          <span>{minutes} min read</span>
          <span className="ml-auto">
            <SaveStatus state={saveState} savedAt={savedAt} />
          </span>
        </div>
      ) : (
        <div className="mb-4 flex flex-wrap items-center gap-2">
          {chrome.type === "post" || chrome.type === "page" ? (
            <Link
              to="/posts"
              search={{ page: 1 }}
              aria-label="Back to posts"
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <ArrowLeft className="h-4 w-4" aria-hidden="true" />
            </Link>
          ) : (
            <Link
              to="/entries/$type"
              params={{ type: chrome.type }}
              aria-label="Back to the list"
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <ArrowLeft className="h-4 w-4" aria-hidden="true" />
            </Link>
          )}

          <div className="min-w-0 flex-1">
            <h1 className="truncate text-lg font-semibold tracking-tight">
              {isNew && title === "" ? `New ${chrome.type === "post" || chrome.type === "page" ? "post" : (typeName ?? "entry")}` : title || "Untitled"}
            </h1>
            <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
              <StatusChip status={isNew ? chrome.status : liveStatus} />
              <span>·</span>
              <span>{words === 1 ? "1 word" : `${words} words`}</span>
              <span>·</span>
              <span>{minutes} min read</span>
              {saveState !== "clean" ? (
                <>
                  <span>·</span>
                  <SaveStatus state={saveState} savedAt={savedAt} />
                </>
              ) : null}
              {unpublished && saveState !== "unsaved" ? (
                <>
                  <span>·</span>
                  <span className="text-amber-600 dark:text-amber-400" data-testid="unpublished-chip">
                    Unpublished changes
                  </span>
                </>
              ) : null}
              {missingAlt > 0 ? (
                <>
                  <span>·</span>
                  <span
                    className="text-amber-600 dark:text-amber-400"
                    title="Screen readers and search engines read alt text; select the image to add it."
                    data-testid="missing-alt"
                  >
                    {missingAlt === 1 ? "1 image without alt text" : `${missingAlt} images without alt text`}
                  </span>
                </>
              ) : null}
            </div>
          </div>

          {/* Desktop actions; phones get the fixed bar below. */}
          <div className="hidden items-center gap-2 lg:flex">
            {!isNew ? (
              <Button variant="outline" size="sm" aria-label="Preview post" onClick={() => void openPreview()}>
                <ExternalLink className="h-4 w-4" aria-hidden="true" />
                Preview
              </Button>
            ) : null}
            <div
              role="radiogroup"
              aria-label="Editor style"
              className="flex items-center gap-0.5 rounded-lg border bg-muted/60 p-0.5"
            >
              <ModeButton
                active={editorMode === "canvas" && !showJson && !showSections}
                label="Canvas"
                hint="Write like a document"
                onClick={() => {
                  setShowJson(false);
                  setShowSections(false);
                  setEditorMode("canvas");
                }}
              >
                <PenLine className="h-3.5 w-3.5" aria-hidden="true" />
              </ModeButton>
              <ModeButton
                active={editorMode === "classic" && !showJson && !showSections}
                label="Classic"
                hint="Edit block fields"
                onClick={() => {
                  setShowJson(false);
                  setShowSections(false);
                  setEditorMode("classic");
                }}
              >
                <Rows3 className="h-3.5 w-3.5" aria-hidden="true" />
              </ModeButton>
              <ModeButton
                active={showJson}
                label="JSON"
                hint="Raw document"
                onClick={() => {
                  setShowSections(false);
                  setShowJson((v) => !v);
                }}
              >
                <Code2 className="h-3.5 w-3.5" aria-hidden="true" />
              </ModeButton>
              {isPage ? (
                <ModeButton
                  active={showSections}
                  label="Sections"
                  hint="Compose this page"
                  onClick={() => {
                    setShowJson(false);
                    setShowSections((v) => !v);
                  }}
                >
                  <LayoutTemplate className="h-3.5 w-3.5" aria-hidden="true" />
                </ModeButton>
              ) : null}
            </div>
            {editorMode === "canvas" && !showJson && !showSections ? (
              <div
                role="radiogroup"
                aria-label="Canvas width"
                className="hidden items-center gap-0.5 rounded-lg border bg-muted/60 p-0.5 xl:flex"
              >
                {(
                  [
                    ["measure", "Measure", "70 characters: easiest to read"],
                    ["wide", "Wide", "110 characters"],
                    ["full", "Full", "Use the whole column"],
                  ] as [CanvasWidth, string, string][]
                ).map(([value, label, hint]) => (
                  <ModeButton
                    key={value}
                    active={canvasWidth === value}
                    label={label}
                    hint={hint}
                    onClick={() => setCanvasWidth(value)}
                  >
                    <span aria-hidden="true" className="inline-block h-3 w-3 border-b-2 border-current" style={{ width: value === "measure" ? 6 : value === "wide" ? 10 : 14 }} />
                  </ModeButton>
                ))}
              </div>
            ) : null}
            <Button
              variant="outline"
              size="sm"
              onClick={() => setFocus(true)}
              aria-label="Focus mode"
              title="Focus mode (Ctrl+Shift+F)"
            >
              <Maximize2 className="h-4 w-4" aria-hidden="true" />
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setSidebarOpen((v) => !v)}
              aria-pressed={sidebarOpen}
              aria-label={sidebarOpen ? "Hide settings" : "Show settings"}
            >
              {sidebarOpen ? (
                <PanelRightClose className="h-4 w-4" aria-hidden="true" />
              ) : (
                <PanelRightOpen className="h-4 w-4" aria-hidden="true" />
              )}
            </Button>
            <Button variant="outline" size="sm" onClick={save} disabled={saving} data-testid="save-post">
              {saveLabel}
            </Button>
            <Button size="sm" onClick={publish} disabled={saving || !canPublish || !can("publish_posts")} data-testid="publish-post">
              {saving ? "Saving…" : primaryLabel}
            </Button>
            {!isNew ? (
              <Button
                variant="outline"
                size="sm"
                onClick={() => void askTrash()}
                aria-label="Move to trash"
              >
                <Trash2 className="h-4 w-4 text-destructive" aria-hidden="true" />
              </Button>
            ) : null}
          </div>
        </div>
      )}

      {postQuery.isError ? (
        <ErrorNote title="Couldn't load this post" error={postQuery.error} />
      ) : null}

      {lock.heldByOther ? (
        <div
          className="mb-4 flex flex-wrap items-center gap-2 rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm"
          role="status"
          data-testid="lock-banner"
        >
          <AlertTriangle className="h-4 w-4 text-amber-600" aria-hidden="true" />
          <span className="min-w-0 flex-1">
            <strong>{lock.holderName ?? "Someone"}</strong> is editing this post
            {lock.seenAgoSecs !== null ? ` (seen ${lock.seenAgoSecs}s ago)` : ""}. Saving is held back so you don't overwrite each other.
          </span>
          <Button size="sm" variant="outline" onClick={lock.takeOver}>
            Take over
          </Button>
        </div>
      ) : null}

      {unpublished && draftOffer === null ? (
        <div
          className="mb-4 flex flex-wrap items-center gap-2 rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm"
          role="status"
          data-testid="unpublished-banner"
        >
          <AlertTriangle className="h-4 w-4 text-amber-600" aria-hidden="true" />
          <span className="min-w-0 flex-1">
            {workingCopyAt !== null
              ? `Unpublished changes, saved ${formatRelative(workingCopyAt)}.`
              : "Unpublished changes."}{" "}
            Readers still see the {liveStatus === "scheduled" ? "scheduled" : "published"} version until you update.
          </span>
          <Button size="sm" variant="ghost" onClick={discardWorkingCopy}>
            Discard
          </Button>
          <Button size="sm" onClick={publish} disabled={saving || !can("publish_posts")}>
            {primaryLabel}
          </Button>
        </div>
      ) : null}

      {draftOffer !== null ? (
        <div
          className="mb-4 flex flex-wrap items-center gap-2 rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm"
          role="status"
          data-testid="draft-offer"
        >
          <AlertTriangle className="h-4 w-4 text-amber-600" aria-hidden="true" />
          <span className="min-w-0 flex-1">
            There's an unsaved draft from {formatRelative(draftOffer.created_at)} that is newer
            than this post.
          </span>
          <Button size="sm" variant="outline" onClick={() => setCompare(draftOffer)}>
            Compare
          </Button>
          <Button size="sm" onClick={takeDraftOffer}>
            Restore draft
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setDraftOffer(null)}>
            Dismiss
          </Button>
        </div>
      ) : null}

      <div
        className={cn(
          "grid gap-4 pb-6 lg:pb-0",
          sidebarOpen && !focus ? "lg:grid-cols-[minmax(0,1fr)_20rem]" : "lg:grid-cols-1",
        )}
      >
        <div
          className="min-w-0 space-y-3"
          style={{ "--vy-canvas-measure": CANVAS_MEASURE[canvasWidth] } as React.CSSProperties}
        >
          <div
            className={cn(
              editorMode === "canvas" &&
                !showJson &&
                "mx-auto max-w-(--vy-canvas-measure,70ch) lg:max-w-[calc(var(--vy-canvas-measure,70ch)+7.5rem)] lg:pl-30",
            )}
          >
            <textarea
              ref={titleRef}
              rows={1}
              value={title}
              onChange={(e) => setTitle(e.target.value.replace(/[\r\n]+/g, " "))}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  focusBody();
                }
              }}
              placeholder="Post title"
              aria-label="Post title"
              data-testid="post-title"
              className="block w-full resize-none overflow-hidden bg-transparent py-1 text-2xl font-semibold leading-tight tracking-tight placeholder:text-muted-foreground/50 focus:outline-hidden focus-visible:ring-0 focus-visible:ring-offset-0 sm:text-3xl"
            />
          </div>
          {surface}
          {hasFields && !focus ? (
            <FieldsPanel
              fields={defs}
              values={fieldValues}
              onChange={setField}
              errors={fieldErrors}
              missing={fieldsMissing}
              disabled={!canEdit}
            />
          ) : null}
        </div>

        {sidebarOpen && !focus ? (
          <>
            <button
              type="button"
              aria-label="Close settings"
              onClick={() => setSidebarOpen(false)}
              className="fixed inset-0 z-40 bg-black/40 backdrop-blur-[1px] lg:hidden"
            />
            <aside
              role="complementary"
              aria-label="Post settings"
              data-testid="post-settings"
              className={cn(
                "min-w-0 space-y-3",
                "max-lg:fixed max-lg:inset-y-0 max-lg:right-0 max-lg:z-50 max-lg:w-[min(24rem,92vw)]",
                "max-lg:overflow-y-auto max-lg:border-l max-lg:bg-background max-lg:p-4 max-lg:shadow-2xl",
              )}
            >
              <div className="flex items-center justify-between lg:hidden">
                <h2 className="text-sm font-semibold">Post settings</h2>
                <button
                  type="button"
                  onClick={() => setSidebarOpen(false)}
                  aria-label="Close settings"
                  className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent"
                >
                  <X className="h-4 w-4" aria-hidden="true" />
                </button>
              </div>
            <PostChromeSidebar
              value={chrome}
              onChange={(p) => setChrome((c) => ({ ...c, ...p }))}
              document={docForSave}
              postId={postId}
            />

            {outline.length >= 2 ? (
              <nav className="overflow-hidden rounded-lg border bg-card" aria-label="Outline">
                <div className="flex items-center gap-2 border-b px-4 py-3 text-sm font-medium">
                  <ListTree className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
                  Outline
                </div>
                <ol className="max-h-64 overflow-y-auto py-1">
                  {outline.map((h, i) => (
                    <li key={`${i}-${h.text}`}>
                      <button
                        type="button"
                        onClick={() => jumpToHeading(i)}
                        style={{ paddingLeft: `${0.75 + (h.level - 2) * 0.75}rem` }}
                        className="block w-full truncate py-1 pr-3 text-left text-xs hover:bg-accent"
                      >
                        {h.text}
                      </button>
                    </li>
                  ))}
                </ol>
              </nav>
            ) : null}

            <div className="overflow-hidden rounded-lg border bg-card">
              <button
                type="button"
                onClick={() => setShowAssist((v) => !v)}
                aria-expanded={showAssist}
                className="flex w-full items-center gap-2 px-4 py-3 text-left text-sm font-medium hover:bg-muted/50"
              >
                <Sparkles className="h-4 w-4 text-primary" aria-hidden="true" />
                Writing assist
              </button>
              {showAssist && ai.loaded && !ai.text ? (
                <p className="px-4 pb-3 text-xs text-muted-foreground" data-testid="assist-unavailable">
                  No text model is set up. Link suggestions still work; for the rest, register one under{" "}
                  <Link to="/models" className="underline">AI models</Link>.
                </p>
              ) : null}
              {showAssist ? (
                <div className="px-3 pb-3">
                  <AssistPanel
                    document={docForSave}
                    vocabulary={tagNames.data ?? []}
                    textAvailable={ai.text}
                    onApplyTitle={(t) => {
                      setTitle(t);
                      notify.success("Title applied");
                    }}
                    onApplyExcerpt={(x) => {
                      setChrome((c) => ({ ...c, excerpt: x }));
                      notify.success("Excerpt applied");
                    }}
                    onApplySeo={(seo) => {
                      setChrome((c) => ({ ...c, seo_title: seo.meta_title, seo_description: seo.meta_description }));
                      notify.success("Search title and description applied", "Save to keep them.");
                    }}
                    onApplyTags={(names) => void applyTags(names)}
                    postId={postId}
                    onApplyLink={(s: LinkSuggestion) => {
                      if (s.phrase === null) {
                        setBlocks((b) => appendFurtherReading(b, s.title, s.url));
                        notify.success("Related link added at the end");
                        return;
                      }
                      const next = linkPhrase(blocks, s.phrase, s.url);
                      if (next === null) {
                        notify.error("Couldn't find that phrase", "The draft changed since the suggestion.");
                        return;
                      }
                      setBlocks(next);
                      notify.success(`Linked “${s.phrase}”`);
                    }}
                  />
                </div>
              ) : null}
            </div>

            <div className="overflow-hidden rounded-lg border bg-card">
              <button
                type="button"
                onClick={() => setShowSerp((v) => !v)}
                aria-expanded={showSerp}
                className="flex w-full items-center gap-2 px-4 py-3 text-left text-sm font-medium hover:bg-muted/50"
              >
                <Sparkles className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
                Search preview
              </button>
              {showSerp ? (
                <div className="px-3 pb-3">
                  <SerpPreview
                    title={chrome.seo_title.trim() !== "" ? chrome.seo_title : title}
                    description={
                      chrome.seo_description.trim() !== ""
                        ? chrome.seo_description
                        : chrome.excerpt
                    }
                    path={publicPath}
                    imageUrl={chrome.featured_media_url !== "" ? chrome.featured_media_url : firstImageUrl(blocks)}
                  />
                </div>
              ) : null}
            </div>

            <div className="overflow-hidden rounded-lg border bg-card">
              <button
                type="button"
                onClick={() => setShowSeo((v) => !v)}
                aria-expanded={showSeo}
                className="flex w-full items-center gap-2 px-4 py-3 text-left text-sm font-medium hover:bg-muted/50"
                data-testid="seo-toggle"
              >
                <Globe className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
                SEO
              </button>
              {showSeo ? (
                <div className="px-3 pb-3">
                  <SeoPanel
                    chrome={chrome}
                    onChange={(p) => setChrome((c) => ({ ...c, ...p }))}
                    title={title}
                    blocks={blocks}
                    postId={postId}
                    isLive={isLive}
                    path={publicPath}
                  />
                </div>
              ) : null}
            </div>

            {!isNew && postId !== undefined ? (
              <LanguageCard postId={postId} />
            ) : null}

            {!isNew ? (
              <div className="overflow-hidden rounded-lg border bg-card">
                <div className="flex items-center gap-2 border-b px-4 py-3 text-sm font-medium">
                  <History className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
                  Revisions
                  {revisions.length > 0 ? (
                    <Chip tone="neutral" dot={false} className="ml-auto">
                      {revisions.length}
                    </Chip>
                  ) : null}
                </div>
                {revisions.length === 0 ? (
                  <p className="px-4 py-3 text-xs text-muted-foreground">
                    Saving creates a snapshot you can return to.
                  </p>
                ) : (
                  <ul className="max-h-64 divide-y overflow-y-auto">
                    {revisions.slice(0, 12).map((r) => (
                      <li
                        key={r.id}
                        className="flex items-center gap-2 px-3 py-2 text-xs"
                      >
                        <span className="min-w-0 flex-1">
                          <span className="block truncate font-medium">
                            {r.title || "Untitled"}
                          </span>
                          <span className="block text-muted-foreground">
                            {formatRelative(r.created_at)}
                            {r.is_autosave ? " · autosave" : ""}
                          </span>
                        </span>
                        <Button
                          variant="ghost"
                          size="sm"
                          className="h-7 shrink-0 px-2 text-[11px]"
                          aria-label={`Compare with revision from ${formatRelative(r.created_at)}`}
                          onClick={() => setCompare(r)}
                        >
                          <GitCompare className="h-3.5 w-3.5" aria-hidden="true" />
                        </Button>
                        <Button
                          variant="outline"
                          size="sm"
                          className="h-7 shrink-0 px-2 text-[11px]"
                          onClick={() => loadRevision(r)}
                        >
                          Load
                        </Button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            ) : null}
            </aside>
          </>
        ) : null}
      </div>

      <Modal
        open={showShortcuts}
        onClose={() => setShowShortcuts(false)}
        title="Keyboard shortcuts"
        description="Most of these work in both editors; the block shortcuts are the canvas's."
        testId="shortcut-sheet"
        footer={
          <Button variant="ghost" onClick={() => setShowShortcuts(false)}>
            Close
          </Button>
        }
      >
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
          {SHORTCUTS.map(([keys, what]) => (
            <React.Fragment key={keys}>
              <dt><kbd className="rounded border bg-muted px-1.5 py-0.5 font-mono text-[11px]">{keys}</kbd></dt>
              <dd className="text-muted-foreground">{what}</dd>
            </React.Fragment>
          ))}
        </dl>
      </Modal>

      {compare !== null ? (
        <RevisionCompare
          revision={compare}
          currentTitle={title}
          currentBlocks={blocks}
          restoring={false}
          onRestore={() => loadRevision(compare)}
          onClose={() => setCompare(null)}
        />
      ) : null}

      {/* Phone and tablet bars — formatting on top when the canvas is open,
          then the primary action one thumb away. Sticky inside the content
          column (not fixed to the viewport) so they never cover the
          navigation sidebar on a tablet. */}
      <div className="sticky bottom-0 z-30 -mx-3 -mb-3 mt-auto border-t bg-card/95 backdrop-blur-sm sm:-mx-4 sm:-mb-4 md:-mx-6 md:-mb-6 lg:hidden">
        {editorMode === "canvas" && !showJson ? (
          <div id={MOBILE_TOOLBAR_SLOT} className="border-b" />
        ) : null}
        <div className="flex flex-wrap items-center gap-2 px-3 py-2.5 pb-[calc(0.625rem+env(safe-area-inset-bottom))]">
          <Button
            variant="outline"
            size="sm"
            onClick={() => setSidebarOpen((v) => !v)}
            aria-pressed={sidebarOpen}
          >
            Settings
          </Button>
          {!isNew ? (
            <Button variant="outline" size="sm" aria-label="Preview post" onClick={() => void openPreview()}>
              <ExternalLink className="h-4 w-4" aria-hidden="true" />
            </Button>
          ) : null}
          <Button
            variant="outline"
            size="sm"
            onClick={() => {
              setShowJson(false);
              setEditorMode(editorMode === "canvas" ? "classic" : "canvas");
            }}
            aria-label={
              editorMode === "canvas"
                ? "Switch to the classic editor"
                : "Switch to the canvas editor"
            }
          >
            {editorMode === "canvas" ? (
              <Rows3 className="h-4 w-4" aria-hidden="true" />
            ) : (
              <PenLine className="h-4 w-4" aria-hidden="true" />
            )}
          </Button>
          <Button variant="outline" size="sm" onClick={save} disabled={saving} aria-label={saveLabel}>
            {saveLabel}
          </Button>
          <Button size="sm" className="ml-auto flex-1" onClick={publish} disabled={saving || !canPublish || !can("publish_posts")}>
            <Check className="h-4 w-4" aria-hidden="true" />
            {saving ? "Saving…" : primaryLabel}
          </Button>
          {!isNew ? (
            <Button
              variant="outline"
              size="sm"
              onClick={() => void askTrash()}
              aria-label="Move to trash"
            >
              <Trash2 className="h-4 w-4 text-destructive" aria-hidden="true" />
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

/**
 * The raw document as text. The pane owns what is typed: a controlled
 * textarea fed straight from the blocks reset to the pretty-printed
 * document on every keystroke that was not yet valid JSON, which is every
 * keystroke in the middle of an edit, so nothing could be typed at all.
 * Valid text is applied as it is typed; the blocks refresh the text only
 * when they changed somewhere else.
 */
export function JsonPane({
  document,
  error,
  onChange,
}: {
  document: unknown;
  error: string | null;
  onChange: (text: string) => void;
}) {
  const pretty = React.useMemo(() => {
    try {
      return JSON.stringify(document, null, 2);
    } catch {
      return "";
    }
  }, [document]);
  const [text, setText] = React.useState(pretty);
  const applied = React.useRef(pretty);
  React.useEffect(() => {
    if (pretty !== applied.current) {
      applied.current = pretty;
      setText(pretty);
    }
  }, [pretty]);
  return (
    <div className="space-y-2">
      <p className="text-xs text-muted-foreground">
        The raw document. Edits here replace the blocks above.
      </p>
      <textarea
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          try {
            applied.current = JSON.stringify(JSON.parse(e.target.value), null, 2);
          } catch {
            // Not valid yet: the text stays as typed and the error shows.
          }
          onChange(e.target.value);
        }}
        rows={22}
        spellCheck={false}
        aria-label="Document JSON"
        data-testid="json-editor"
        className="w-full rounded-lg border bg-muted/40 p-3 font-mono text-xs focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
      />
      {error !== null ? (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/**
 * Language & translations: declare the entry's language and link it to a
 * translation by slug. Saved through its own endpoint, so the entry
 * itself stays untouched; hreflang links appear once two published
 * members share a group.
 */
function LanguageCard({ postId }: { postId: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = React.useState(false);
  const [lang, setLang] = React.useState<string | null>(null);
  const [linkSlug, setLinkSlug] = React.useState("");
  const current = useQuery({
    queryKey: ["language", postId],
    queryFn: () => api.getLanguage(postId),
    enabled: open,
  });
  const save = useMutation({
    mutationFn: () =>
      api.putLanguage(postId, {
        lang: (lang ?? current.data?.lang ?? "").trim(),
        link_slug: linkSlug.trim() || undefined,
      }),
    onSuccess: () => {
      setLinkSlug("");
      void queryClient.invalidateQueries({ queryKey: ["language", postId] });
      notify.success("Language saved");
    },
    onError: (e) => notify.error("Couldn't save the language", e),
  });
  const shownLang = lang ?? current.data?.lang ?? "";
  const group = current.data?.group ?? [];
  return (
    <div className="overflow-hidden rounded-lg border bg-card">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex w-full items-center gap-2 px-4 py-3 text-left text-sm font-medium hover:bg-muted/50"
      >
        <Globe className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
        Language
      </button>
      {open ? (
        <div className="space-y-3 px-4 pb-4 text-sm">
          <label className="block">
            <span className="text-xs font-medium text-muted-foreground">
              Language tag (e.g. en, pt-BR)
            </span>
            <input
              value={shownLang}
              onChange={(e) => setLang(e.target.value)}
              placeholder="en"
              className="mt-1 w-full rounded border bg-background px-2 py-1.5"
            />
          </label>
          <label className="block">
            <span className="text-xs font-medium text-muted-foreground">
              Translation of (slug, optional)
            </span>
            <input
              value={linkSlug}
              onChange={(e) => setLinkSlug(e.target.value)}
              placeholder="the-same-entry-in-another-language"
              className="mt-1 w-full rounded border bg-background px-2 py-1.5"
            />
          </label>
          <button
            type="button"
            disabled={save.isPending}
            onClick={() => save.mutate()}
            className="rounded border px-3 py-1.5 text-sm hover:bg-accent"
          >
            {save.isPending ? "Saving…" : "Save language"}
          </button>
          {group.length > 1 ? (
            <div>
              <p className="text-xs font-medium text-muted-foreground">Translation group</p>
              <ul className="mt-1 space-y-1">
                {group.map((g) => (
                  <li key={g.id} className="flex items-baseline justify-between gap-2">
                    <span className="min-w-0 truncate">{g.title}</span>
                    <span className="text-xs text-muted-foreground">{g.lang}</span>
                  </li>
                ))}
              </ul>
              <p className="mt-1 text-xs text-muted-foreground">
                Published members link each other with hreflang automatically.
              </p>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/**
 * Field values in a fixed key order, so the same values always compare
 * equal whichever order the server or the editor put them in.
 */
export function fieldsKey(values: FieldValues | null | undefined): string {
  const v = values ?? {};
  return JSON.stringify(Object.keys(v).sort().map((k) => [k, v[k]]));
}

/** What autosave and "unsaved" compare: the prose and the field values. */
function snapshotOf(title: string, blocks: Block[], excerpt: string, fields: FieldValues): string {
  return JSON.stringify({ title, blocks, excerpt, fields: fieldsKey(fields) });
}

/** The values the server sent, as the editor holds them. */
function fieldsOf(post: unknown): FieldValues {
  const f = (post as { fields?: unknown } | undefined)?.fields;
  return f !== null && typeof f === "object" && !Array.isArray(f) ? { ...(f as FieldValues) } : {};
}

/** URL of the first library image in the document, for the social card. */
function firstImageUrl(blocks: Block[]): string | null {
  for (const b of blocks) {
    if ((b.kind === "image" || b.kind === "cover") && typeof b.attrs.mediaId !== "undefined") {
      return `/api/v1/media/${String(b.attrs.mediaId)}/raw?variant=medium`;
    }
    if (typeof b.attrs.url === "string" && (b.kind === "image" || b.kind === "cover")) {
      return b.attrs.url;
    }
    const nested = firstImageUrl(b.children);
    if (nested !== null) return nested;
  }
  return null;
}
