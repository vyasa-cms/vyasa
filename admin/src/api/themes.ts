import { previewFailure } from "./client";
import { parseJsonPreservingIds } from "./json-bigint";

/* ------------------------------------------------------------ themes */

export interface ThemeSummary {
  id: string;
  name: string;
  /** The site's own sequence: every install or studio publish advances it. */
  version: number;
  is_active: boolean;
  /** The package manifest's version, when the row came from a package. */
  package_version?: number | null;
}

/** One colour role: a light value plus an optional dark override. */
export interface ColorSlot {
  light: string;
  dark?: string | null;
}

/** The seven roles Vyasa themes understand. */
export interface ColorPalette {
  bg: ColorSlot;
  surface: ColorSlot;
  text: ColorSlot;
  text_muted: ColorSlot;
  primary: ColorSlot;
  on_primary: ColorSlot;
  border: ColorSlot;
}

/** A built-in stack or a custom family name. */
export type FontChoice = "system_ui" | "serif" | "mono" | { custom: string };

export type ScaleRatio =
  | "major_second"
  | "minor_third"
  | "major_third"
  | "perfect_fourth"
  | "golden"
  | { custom: number };

export interface FontFace {
  family: string;
  src?: string | null;
}

/** Mirrors `vyasa_themes::TokenSet` (docs/theme-token-schema.json). */
export interface TokenSet {
  version: number;
  colors: ColorPalette;
  typography: {
    heading: FontChoice;
    body: FontChoice;
    base_size_px: number;
    scale_ratio: ScaleRatio;
    font_faces: FontFace[];
  };
  spacing: { unit_px: number; section_scale: number[] };
  radius_px: number;
  shadow: "none" | "small" | "medium" | "large";
  layout: {
    content_width_px: number;
    sidebar_width_px: number;
    density: "compact" | "comfortable" | "spacious";
    /** Width below which columns collapse and type steps down. */
    breakpoint_sm_px: number;
    /** Width below which the sidebar drops under the content. */
    breakpoint_md_px: number;
  };
  direction: "ltr" | "rtl";
}

export type TemplateType =
  | "index"
  | "single"
  | "archive"
  | "page"
  | "search"
  | "not-found";

export const TEMPLATE_TYPES: TemplateType[] = [
  "index",
  "single",
  "archive",
  "page",
  "search",
  "not-found",
];

/** A colour in a scope: a hex literal, or `"$role"` referencing a theme role. */
export type ColorRef = string;

/**
 * Colour and spacing overrides applied to one section and everything inside
 * it. This is what expresses a dark band between two light ones without
 * anyone writing CSS.
 */
export interface StyleScope {
  bg?: ColorRef;
  surface?: ColorRef;
  text?: ColorRef;
  text_muted?: ColorRef;
  border?: ColorRef;
  primary?: ColorRef;
  on_primary?: ColorRef;
  padding_y?: number;
  contained?: boolean;
  motion?: "fade" | "rise";
}

/** The roles a scope may override, in the order the compiler emits them. */
export const SCOPE_ROLES = [
  "bg",
  "surface",
  "text",
  "text_muted",
  "border",
  "primary",
  "on_primary",
] as const;

export type ScopeRole = (typeof SCOPE_ROLES)[number];

/**
 * One node in a template's composition tree.
 *
 * `children` is only legal on container kinds; the server rejects it
 * elsewhere. Ids are unique across the whole tree, not just among siblings.
 */
/** A screen size a section may be withheld from. */
export type Screen = "mobile" | "desktop";

export interface Section {
  id: string;
  kind: string;
  settings?: Record<string, unknown>;
  children?: Section[];
  scope?: StyleScope;
  /** Absent means shown everywhere. */
  hide_on?: Screen;
  /** How wide the content runs; absent means the theme's content width. */
  width?: "narrow" | "content" | "wide" | "full";
}

export type Layout = Record<TemplateType, Section[]>;

export interface ThemeTokens {
  id: string;
  name: string;
  version: number;
  is_active: boolean;
  tokens: TokenSet;
  layout: Layout;
}

/* ------------------------------------------------------------ studio */

export type DraftStatus = "ready" | "generating" | "failed";

export interface DraftSummary {
  id: string;
  name: string;
  base_theme_id: string | null;
  status: DraftStatus;
  status_note: string | null;
  revision: number;
  updated_at: string;
  colors: ColorPalette | null;
}

export interface Diagnostic {
  level: "error" | "warning";
  path: string;
  message: string;
}

export interface Draft {
  id: string;
  name: string;
  base_theme_id: string | null;
  status: DraftStatus;
  status_note: string | null;
  tokens: TokenSet;
  layout: Layout;
  templates: Record<string, string>;
  assets: ThemeAssets | null;
  revision: number;
  created_at: string;
  updated_at: string;
  warnings: Diagnostic[];
}

/** One edit. The server validates the batch as a whole. */
export type StudioOp =
  | { op: "set_tokens"; tokens: TokenSet }
  | { op: "patch_tokens"; patch: unknown }
  | { op: "set_layout"; template: TemplateType; blocks: Section[] }
  | { op: "set_template"; name: string; source: string }
  | { op: "remove_template"; name: string }
  | { op: "set_assets"; css: string; js: string };

/** CSS and JavaScript a theme ships with itself. */
export interface ThemeAssets {
  css: string;
  js: string;
}

/** A JSON Schema fragment for a block's settings. */
/**
 * A settings schema, as the server publishes it.
 *
 * Recursive on purpose: the marketing sections take arrays of records, so a
 * property is itself a schema rather than a flat scalar descriptor.
 */
export interface SettingsSchema {
  type?: string;
  format?: string;
  /** Element schema, when this describes an array. */
  items?: SettingsSchema;
  maxItems?: number;
  required?: string[];
  minimum?: number;
  maximum?: number;
  maxLength?: number;
  minLength?: number;
  enum?: string[];
  description?: string;
  properties?: Record<string, SettingsSchema>;
}

/** One content source a binding may draw from. */
export interface ContentSource {
  slug: string;
  singular: string;
  plural: string;
  public: boolean;
  has_archive: boolean;
  /** Registered by a plugin (and gone while it is disabled). */
  plugin: boolean;
  /** Published entries right now. */
  count: number;
}

export interface Vocabulary {
  static_regions: { kind: string; description: string }[];
  blocks: {
    kind: string;
    description: string;
    settings_schema: SettingsSchema;
    /** Whether the kind may hold nested sections. */
    container?: boolean;
    /** Insert-library grouping: structure, marketing, content, navigation. */
    category?: string;
    /** Starter settings for a freshly inserted instance. */
    sample?: Record<string, unknown>;
    /** String settings the canvas edits in place. */
    inline?: string[];
    /** Owning plugin's name, for kinds a plugin registered. */
    plugin?: string;
  }[];
  templates: TemplateType[];
  template_files: { name: string; builtin_source: string }[];
  token_schema: unknown;
  /** Live content sources for bindings; absent on older servers. */
  sources?: ContentSource[];
}

export interface Revision {
  seq: number;
  note: string;
  source: "you" | "assistant" | "start" | "revert";
  created_at: string;
}

export interface RevisionDetail extends Revision {
  tokens: TokenSet;
  layout: Layout;
  templates: Record<string, string>;
}

export interface StudioMessage {
  id: string;
  role: "you" | "assistant";
  text: string;
  revision: number | null;
  /** What this reply would change, until someone accepts it. */
  proposal: {
    changes: string[] | null;
    base: number | null;
    /** The agent's work log: how the proposal came to be. */
    steps?: { thought: string; tool: string; observation: string }[] | null;
  } | null;
  created_at: string;
}

async function json<T>(response: Response, what: string): Promise<T> {
  const text = await response.text();
  if (!response.ok) {
    let body: { message?: string } | null = null;
    try {
      body = JSON.parse(text) as { message?: string };
    } catch {
      // non-JSON error body
    }
    throw new Error(body?.message ?? `${what} failed (${response.status})`);
  }
  // Theme ids are 64-bit like every other id, so they must not go through the
  // built-in parser. See ./json-bigint.ts.
  return parseJsonPreservingIds<T>(text);
}

async function empty(response: Response, what: string): Promise<void> {
  if (response.ok) return;
  const body = (await response.json().catch(() => null)) as
    | { message?: string }
    | null;
  throw new Error(body?.message ?? `${what} failed (${response.status})`);
}

const jsonInit = (method: string, body?: unknown): RequestInit => ({
  method,
  credentials: "same-origin",
  headers: { "content-type": "application/json" },
  ...(body === undefined ? {} : { body: JSON.stringify(body) }),
});

export const listThemes = (): Promise<ThemeSummary[]> =>
  fetch("/api/v1/themes", { credentials: "same-origin" }).then((r) =>
    json<ThemeSummary[]>(r, "Loading themes"),
  );

/** Design tokens and layout for one stored version. */
export const themeTokens = (id: string): Promise<ThemeTokens> =>
  fetch(`/api/v1/themes/${id}/tokens`, { credentials: "same-origin" }).then((r) =>
    json<ThemeTokens>(r, "Loading theme tokens"),
  );

export const activate = (id: string): Promise<void> =>
  fetch(`/api/v1/themes/${id}/activate`, jsonInit("POST")).then((r) =>
    empty(r, "Activation"),
  );

export const deleteTheme = (id: string): Promise<void> =>
  fetch(`/api/v1/themes/${id}`, jsonInit("DELETE")).then((r) =>
    empty(r, "Deleting the theme"),
  );

export const rollbackTheme = (name: string): Promise<ThemeSummary> =>
  fetch(`/api/v1/themes/${encodeURIComponent(name)}/rollback`, jsonInit("POST")).then(
    (r) => json<ThemeSummary>(r, "Rolling back"),
  );

/** Uploads a `.vytheme` package. Installs only; nothing goes live. */
export const installTheme = (file: File, signature?: string): Promise<ThemeSummary> => {
  const form = new FormData();
  form.append("file", file);
  // A theme that carries a script must be signed; the .sig travels as text.
  if (signature) form.append("signature", signature.trim());
  return fetch("/api/v1/themes", {
    method: "POST",
    credentials: "same-origin",
    body: form,
  }).then((r) => json<ThemeSummary>(r, "Installing the theme"));
};

/** One picture or font an installed version bundles. */
export interface ThemeFile {
  path: string;
  content_type: string;
  sha256: string;
  size: number;
  url: string;
}

export const listThemeFiles = (id: string): Promise<ThemeFile[]> =>
  fetch(`/api/v1/themes/${id}/files`, { credentials: "same-origin" }).then((r) =>
    json<ThemeFile[]>(r, "Loading the theme's files"),
  );

/**
 * Adds or replaces one bundled file. The path is `images/…` or `fonts/…`;
 * the server holds it to the same rules a package is.
 */
export const putThemeFile = (id: string, path: string, file: File): Promise<ThemeFile> =>
  fetch(`/api/v1/themes/${id}/files/${path.split("/").map(encodeURIComponent).join("/")}`, {
    method: "PUT",
    credentials: "same-origin",
    headers: { "content-type": "application/octet-stream" },
    body: file,
  }).then((r) => json<ThemeFile>(r, "Uploading the file"));

export const deleteThemeFile = (id: string, path: string): Promise<void> =>
  fetch(
    `/api/v1/themes/${id}/files/${path.split("/").map(encodeURIComponent).join("/")}`,
    jsonInit("DELETE"),
  ).then((r) => empty(r, "Removing the file"));

/**
 * The path a chosen file gets under `assets/`: its own name, lowercased,
 * with anything the server would refuse replaced by a dash.
 */
export const bundledPathFor = (dir: "images" | "fonts", fileName: string): string => {
  const clean = fileName
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^\.+/, "");
  return `${dir}/${clean === "" ? "file" : clean}`;
};

/** Preview of an installed version, for a new tab. */
export const themePreviewUrl = (id: string, path = "/"): string =>
  `/preview/theme/${id}?path=${encodeURIComponent(path)}`;

/* studio */

export const vocabulary = (): Promise<Vocabulary> =>
  fetch("/api/v1/themes/vocabulary", { credentials: "same-origin" }).then((r) =>
    json<Vocabulary>(r, "Loading the theme vocabulary"),
  );

export const listDrafts = (): Promise<DraftSummary[]> =>
  fetch("/api/v1/themes/drafts", { credentials: "same-origin" }).then((r) =>
    json<DraftSummary[]>(r, "Loading drafts"),
  );

export const createDraft = (input: {
  name?: string;
  base_theme_id?: string;
}): Promise<Draft> =>
  fetch("/api/v1/themes/drafts", jsonInit("POST", input)).then((r) =>
    json<Draft>(r, "Starting a draft"),
  );

export const getDraft = (id: string): Promise<Draft> =>
  fetch(`/api/v1/themes/drafts/${id}`, { credentials: "same-origin" }).then((r) =>
    json<Draft>(r, "Loading the draft"),
  );

export const renameDraft = (id: string, name: string): Promise<Draft> =>
  fetch(`/api/v1/themes/drafts/${id}`, jsonInit("PATCH", { name })).then((r) =>
    json<Draft>(r, "Renaming the draft"),
  );

export const deleteDraft = (id: string): Promise<void> =>
  fetch(`/api/v1/themes/drafts/${id}`, jsonInit("DELETE")).then((r) =>
    empty(r, "Deleting the draft"),
  );

/**
 * Applies edits as one revision. A 400 carries every diagnostic, one per
 * line (`error: colors.primary.light: …`), and leaves the draft untouched.
 */
export const applyOps = (id: string, ops: StudioOp[], note?: string): Promise<Draft> =>
  fetch(`/api/v1/themes/drafts/${id}/ops`, jsonInit("POST", { ops, note })).then((r) =>
    json<Draft>(r, "Saving the change"),
  );

export const listRevisions = (id: string): Promise<Revision[]> =>
  fetch(`/api/v1/themes/drafts/${id}/revisions`, { credentials: "same-origin" }).then(
    (r) => json<Revision[]>(r, "Loading history"),
  );

export const getRevision = (id: string, seq: number): Promise<RevisionDetail> =>
  fetch(`/api/v1/themes/drafts/${id}/revisions/${seq}`, {
    credentials: "same-origin",
  }).then((r) => json<RevisionDetail>(r, "Loading the revision"));

export const revertDraft = (id: string, seq: number): Promise<Draft> =>
  fetch(`/api/v1/themes/drafts/${id}/revert`, jsonInit("POST", { seq })).then((r) =>
    json<Draft>(r, "Going back"),
  );

export const publishDraft = (
  id: string,
  input: { name: string; activate: boolean },
): Promise<ThemeSummary> =>
  fetch(`/api/v1/themes/drafts/${id}/publish`, jsonInit("POST", input)).then((r) =>
    json<ThemeSummary>(r, "Publishing"),
  );

/** Preview of a saved draft, for a new tab. */
export const draftPreviewUrl = (id: string, path = "/"): string =>
  `/api/v1/themes/drafts/${id}/preview?path=${encodeURIComponent(path)}`;

/**
 * Renders documents that have not been saved. Resolves to HTML; rejects
 * with the renderer's own message when the documents cannot render.
 */
export const previewCandidate = async (input: {
  base_theme_id?: string | null;
  tokens: TokenSet;
  layout: Layout;
  templates: Record<string, string>;
  assets: ThemeAssets;
  path: string;
}): Promise<string> => {
  const r = await fetch("/api/v1/themes/preview", jsonInit("POST", input));
  const text = await r.text();
  if (!r.ok) throw previewFailure(r.status, text);
  return text;
};

/**
 * Asks the assistant. Resolves once the request is queued: the draft turns
 * `generating`, and the reply (and any new revision) lands a little later.
 */
export const chat = (
  id: string,
  message: string,
  mediaIds: string[] = [],
): Promise<{ message_id: string; status: DraftStatus }> =>
  fetch(
    `/api/v1/themes/drafts/${id}/chat`,
    jsonInit("POST", { message, media_ids: mediaIds }),
  ).then((r) => json<{ message_id: string; status: DraftStatus }>(r, "Asking the assistant"));

export const listMessages = (id: string): Promise<StudioMessage[]> =>
  fetch(`/api/v1/themes/drafts/${id}/messages`, { credentials: "same-origin" }).then(
    (r) => json<StudioMessage[]>(r, "Loading the conversation"),
  );

/** Writes the documents a reply proposed. The only thing that commits. */
export const acceptProposal = (draftId: string, messageId: string): Promise<Draft> =>
  fetch(
    `/api/v1/themes/drafts/${draftId}/messages/${messageId}/accept`,
    jsonInit("POST"),
  ).then((r) => json<Draft>(r, "Applying the change"));

/* ----------------------------------------------------- contrast checking */

function channel(component: number): number {
  const c = component / 255;
  return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function luminance(hex: string): number | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (m?.[1] === undefined) return null;
  const int = parseInt(m[1], 16);
  return (
    0.2126 * channel((int >> 16) & 255) +
    0.7152 * channel((int >> 8) & 255) +
    0.0722 * channel(int & 255)
  );
}

/** WCAG contrast ratio, or null when either colour can't be parsed. */
export function contrastRatio(a: string, b: string): number | null {
  const la = luminance(a);
  const lb = luminance(b);
  if (la === null || lb === null) return null;
  const [hi, lo] = la > lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

export function slotValue(slot: ColorSlot | undefined, dark: boolean): string {
  if (slot === undefined) return "#000000";
  if (dark) return slot.dark ?? slot.light;
  return slot.light;
}

/** Expands a 3-digit hex so `<input type="color">` accepts it. */
export function normalizeHex(value: string): string {
  const v = value.trim().toLowerCase();
  const short = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/.exec(v);
  if (short !== null) {
    const [, r, g, b] = short;
    return `#${r}${r}${g}${g}${b}${b}`;
  }
  return v;
}
