import { useUnsavedChanges } from "@/lib/use-unsaved-changes";
import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type RoleResponse, type SourceInfo } from "@/api/client";
import { Button, buttonVariants } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { MediaPicker } from "@/components/editor/MediaPicker";
import { ErrorNote, Field, PageHeader, Panel, Skeleton } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { useConfirm } from "@/components/ui/dialog";
import { MonacoPane } from "@/components/editor/MonacoPane";
import { MailRelayPanel } from "@/components/MailRelayPanel";
import { ExportImportPanel } from "@/components/ExportImportPanel";
import { useCapabilities, CAPABILITY_ORDER } from "@/lib/capabilities";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/settings/")({
  component: SettingsPage,
});

type Kind =
  | "text"
  | "url"
  | "number"
  | "css"
  | "toggle"
  | "select"
  | "media"
  | "timezone"
  | "date_format"
  | "permalink"
  | "keys"
  | "money"
  | "role_select";

interface SettingKey {
  key: string;
  label: string;
  hint?: string;
  placeholder?: string;
  kind: Kind;
  /** Unit shown after a numeric field. */
  unit?: string;
  options?: { value: string; label: string }[];
  /** The wizard step this belongs to, for "rerun this step". */
}

interface Section {
  id: string;
  title: string;
  description: string;
  /** The setup wizard step that covers this section. */
  step?: string;
  keys: SettingKey[];
}

/** A control's width follows what it holds, not the monitor. */
const WIDTH: Record<Kind, string> = {
  text: "max-w-[40ch]",
  url: "max-w-[60ch]",
  number: "max-w-[12ch]",
  money: "max-w-[14ch]",
  css: "",
  toggle: "",
  select: "max-w-[40ch]",
  media: "",
  timezone: "max-w-[40ch]",
  date_format: "max-w-[60ch]",
  permalink: "max-w-[60ch]",
  keys: "max-w-[70ch]",
  role_select: "max-w-[40ch]",
};

/**
 * A media id behind a picture: shows what is chosen, opens the media
 * library to change it, clears back to none. The option stores only the
 * id; everything visual is resolved here.
 */
function MediaIdField({ id, value, onChange }: { id: string; value: string; onChange: (next: string) => void }) {
  const [picking, setPicking] = React.useState(false);
  const current = useQuery({
    queryKey: ["media", "by-id", value],
    queryFn: () => api.getMedia(value),
    enabled: value !== "",
    retry: false,
  });
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-3">
        {value !== "" && current.data !== undefined ? (
          <img src={`/api/v1/media/${current.data.id}/raw`} alt={current.data.alt ?? ""} className="h-10 w-10 rounded-md border bg-muted/40 object-contain" />
        ) : (
          <span className="grid h-10 w-10 place-items-center rounded-md border border-dashed text-[10px] text-muted-foreground">none</span>
        )}
        <button id={id} type="button" onClick={() => setPicking((v) => !v)} className="rounded-md border px-2.5 py-1.5 text-xs font-medium hover:bg-accent">
          {picking ? "Close" : value === "" ? "Choose…" : "Change…"}
        </button>
        {value !== "" ? (
          <button type="button" onClick={() => onChange("")} className="text-xs text-muted-foreground underline hover:text-foreground">Remove</button>
        ) : null}
      </div>
      {picking ? (
        <div className="rounded-md border p-2">
          <MediaPicker accept="image" onPick={(m) => { onChange(String(m.id)); setPicking(false); }} />
        </div>
      ) : null}
    </div>
  );
}

/** A switch: state by position and colour, the label beside it. */
function Switch({ id, checked, onChange, label }: { id: string; checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <label className="inline-flex cursor-pointer items-center gap-2 text-sm">
      <input id={id} type="checkbox" role="switch" className="peer sr-only" checked={checked} aria-checked={checked} onChange={(e) => onChange(e.target.checked)} aria-label={label} />
      <span aria-hidden="true" className={cn("relative inline-block h-5 w-9 rounded-full border transition-colors peer-focus-visible:ring-2 peer-focus-visible:ring-ring", checked ? "border-primary bg-primary" : "bg-muted")}>
        <span className={cn("absolute top-0.5 h-3.5 w-3.5 rounded-full bg-background shadow-sm transition-transform", checked ? "translate-x-[1.1rem]" : "translate-x-0.5")} />
      </span>
      <span className="text-muted-foreground">{checked ? "On" : "Off"}</span>
    </label>
  );
}

const DATE_PRESETS: { value: string; label: string }[] = [
  { value: "%b %e, %Y", label: "Aug 29, 2026" },
  { value: "%e %B %Y", label: "29 August 2026" },
  { value: "%B %e, %Y", label: "August 29, 2026" },
  { value: "%Y-%m-%d", label: "2026-08-29" },
  { value: "%d/%m/%Y", label: "29/08/2026" },
  { value: "%m/%d/%Y", label: "08/29/2026" },
];

/** A near-enough strftime for the preview; the server renders the real one. */
export function previewDate(pattern: string, date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  const months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
  const days = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
  return pattern.replace(/%(-?)([a-zA-Z%])/g, (m, flag: string, c: string) => {
    switch (c) {
      case "Y": return String(date.getFullYear());
      case "y": return pad(date.getFullYear() % 100);
      case "m": return flag ? String(date.getMonth() + 1) : pad(date.getMonth() + 1);
      case "d": return flag ? String(date.getDate()) : pad(date.getDate());
      case "e": return String(date.getDate());
      case "B": return months[date.getMonth()] ?? "";
      case "b": case "h": return (months[date.getMonth()] ?? "").slice(0, 3);
      case "A": return days[date.getDay()] ?? "";
      case "a": return (days[date.getDay()] ?? "").slice(0, 3);
      case "H": return pad(date.getHours());
      case "M": return pad(date.getMinutes());
      case "j": return String(Math.floor((date.getTime() - new Date(date.getFullYear(), 0, 0).getTime()) / 86_400_000));
      case "%": return "%";
      default: return m;
    }
  });
}

function DateFormatField({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void }) {
  const preset = DATE_PRESETS.find((p) => p.value === value);
  const [custom, setCustom] = React.useState(value !== "" && preset === undefined);
  return (
    <div className="space-y-2">
      <select
        id={id}
        value={custom ? "custom" : (preset?.value ?? DATE_PRESETS[0]?.value ?? "")}
        onChange={(e) => {
          if (e.target.value === "custom") setCustom(true);
          else { setCustom(false); onChange(e.target.value); }
        }}
        className="h-9 w-full rounded-md border bg-background px-2 text-sm"
      >
        {DATE_PRESETS.map((p) => <option key={p.value} value={p.value}>{previewDate(p.value)}</option>)}
        <option value="custom">Custom pattern…</option>
      </select>
      {custom ? (
        <div className="flex items-center gap-3">
          <Input aria-label="Custom date pattern" value={value} placeholder="%b %e, %Y" onChange={(e) => onChange(e.target.value)} className="font-mono text-sm" />
          <span className="whitespace-nowrap text-sm text-muted-foreground">→ {previewDate(value || "%b %e, %Y")}</span>
        </div>
      ) : null}
    </div>
  );
}

const PERMALINK_PRESETS: { value: string; label: string }[] = [
  { value: "/post/{slug}", label: "Default" },
  { value: "/{slug}", label: "Post name" },
  { value: "/{year}/{month}/{slug}", label: "Year and month" },
  { value: "/{year}/{slug}", label: "Year" },
  { value: "/{type}/{slug}", label: "Type and name" },
];

function permalinkPreview(pattern: string): string {
  return pattern.replace("{year}", "2026").replace("{month}", "08").replace("{slug}", "hello-world").replace("{type}", "post");
}

function PermalinkField({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void }) {
  const effective = value === "" ? "/post/{slug}" : value;
  const preset = PERMALINK_PRESETS.find((p) => p.value === effective);
  const [custom, setCustom] = React.useState(preset === undefined);
  return (
    <div className="space-y-2">
      <select
        id={id}
        value={custom ? "custom" : effective}
        onChange={(e) => {
          if (e.target.value === "custom") setCustom(true);
          else { setCustom(false); onChange(e.target.value); }
        }}
        className="h-9 w-full rounded-md border bg-background px-2 text-sm"
      >
        {PERMALINK_PRESETS.map((p) => <option key={p.value} value={p.value}>{p.label} · {permalinkPreview(p.value)}</option>)}
        <option value="custom">Custom…</option>
      </select>
      {custom ? (
        <Input aria-label="Custom permalink pattern" value={value} placeholder="/{year}/{slug}" onChange={(e) => onChange(e.target.value)} className="font-mono text-sm" />
      ) : null}
      <p className="text-xs text-muted-foreground">Example: <code>{permalinkPreview(effective)}</code>. Tokens: {"{year} {month} {slug} {type}"}.</p>
    </div>
  );
}

function timezones(): string[] {
  try {
    const list = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.("timeZone");
    if (list && list.length > 0) return list;
  } catch { /* older engines */ }
  return ["UTC", "Europe/London", "Europe/Berlin", "America/New_York", "America/Los_Angeles", "Asia/Kolkata", "Asia/Tokyo", "Australia/Sydney"];
}

function TimezoneField({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void }) {
  const zones = React.useMemo(timezones, []);
  const local = Intl.DateTimeFormat().resolvedOptions().timeZone;
  return (
    <div className="space-y-1">
      <select id={id} value={value === "" ? "UTC" : value} onChange={(e) => onChange(e.target.value)} className="h-9 w-full rounded-md border bg-background px-2 text-sm">
        {zones.includes(value) || value === "" ? null : <option value={value}>{value}</option>}
        {zones.map((z) => <option key={z} value={z}>{z}</option>)}
      </select>
      {local && local !== value ? (
        <button type="button" className="text-xs text-primary underline-offset-2 hover:underline" onClick={() => onChange(local)}>Use this browser's zone ({local})</button>
      ) : null}
    </div>
  );
}

/**
 * Capabilities that make a role too powerful to be handed to anyone who
 * merely fills out a public form: the same seven the server refuses at save
 * time and falls back away from at run time (`FORBIDDEN_DEFAULT_ROLE_CAPS`
 * in `vyasa-core`). Running the site, and power over other people's work or
 * comments or the categories everyone files under: a stranger gets at most
 * an author's power over their own work.
 */
const REGISTRATION_INELIGIBLE_CAPS = [
  "manage_users", "manage_options", "manage_plugins", "manage_themes", "edit_others", "moderate_comments", "manage_categories",
];

/**
 * The role a self-registered account starts with: any built-in or custom
 * role that holds none of `REGISTRATION_INELIGIBLE_CAPS`. The
 * currently saved value is always offered too, even if it has since
 * become ineligible (widened, or the role deleted) — the run-time fallback
 * to `subscriber` is the server's job, not something this control should
 * paper over by silently swapping the value.
 */
function RoleSelectField({ id, value, onChange, roles }: { id: string; value: string; onChange: (v: string) => void; roles: RoleResponse[] }) {
  const eligible = roles.filter((r) => !REGISTRATION_INELIGIBLE_CAPS.some((c) => r.capabilities.includes(c)));
  const effective = value === "" ? "subscriber" : value;
  const offered = eligible.some((r) => r.slug === effective);
  return (
    <select id={id} value={effective} onChange={(e) => onChange(e.target.value)} className="h-9 w-full rounded-md border bg-background px-2 text-sm">
      {!offered ? <option value={effective}>{roles.find((r) => r.slug === effective)?.name ?? effective} (not eligible)</option> : null}
      {eligible.map((r) => <option key={r.slug} value={r.slug}>{r.name}</option>)}
    </select>
  );
}

/** One key per line; blank lines dropped. Stored as a JSON list. */
function KeysField({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void }) {
  return (
    <textarea
      id={id}
      rows={3}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder="one hex ed25519 public key per line"
      className="w-full resize-y rounded-md border border-input bg-background p-2 font-mono text-xs placeholder:text-muted-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
    />
  );
}

const SECTIONS: Section[] = [
  {
    id: "identity",
    title: "Site identity",
    description: "How your site introduces itself to readers and search engines.",
    step: "site",
    keys: [
      { key: "site_title", label: "Site title", hint: "Appears in the browser tab and in search results.", placeholder: "Vyasa", kind: "text" },
      { key: "site_tagline", label: "Tagline", hint: "One line describing what the site is about.", placeholder: "A fresh CMS in Rust", kind: "text" },
      { key: "site_url", label: "Site address", hint: "Your public URL. Canonical links, social cards and feeds use it. Checked against this server before saving.", placeholder: "https://example.com", kind: "url" },
      { key: "site_language", label: "Language", hint: "A BCP 47 tag for the html element: en, de, pt-BR.", placeholder: "en", kind: "text" },
      { key: "site_logo_media_id", label: "Logo", hint: "Shown in the site header in place of the wordmark; the site title stays as its alt text.", kind: "media" },
      { key: "site_favicon_media_id", label: "Favicon", hint: "The browser-tab icon. A square PNG works everywhere.", kind: "media" },
    ],
  },
  {
    id: "reading",
    title: "Reading and writing",
    description: "How posts are addressed and how many appear at once.",
    step: "content",
    keys: [
      { key: "permalink_pattern", label: "Permalinks", hint: "Changing this later breaks links people have already shared; redirects are added for published posts.", kind: "permalink" },
      { key: "posts_per_page", label: "Posts per page", hint: "1 to 100.", placeholder: "10", kind: "number", unit: "posts" },
    ],
  },
  {
    id: "locale",
    title: "Dates and locale",
    description: "When things happen and how the date reads.",
    step: "site",
    keys: [
      { key: "timezone", label: "Timezone", hint: "Scheduling and published dates use it.", kind: "timezone" },
      { key: "date_format", label: "Date format", hint: "How published dates read on the site.", kind: "date_format" },
    ],
  },
  {
    id: "mail",
    title: "Comments and mail",
    description: "What happens when a visitor comments, and what goes out by email.",
    step: "mail",
    keys: [
      {
        key: "comment_moderation",
        label: "Moderation",
        hint: "Held comments wait in the Comments queue; the visitor is told theirs is awaiting review.",
        kind: "select",
        options: [
          { value: "require_first", label: "Hold a commenter's first comment" },
          { value: "auto_approve", label: "Publish every comment straight away" },
          { value: "require_all", label: "Hold every comment for review" },
        ],
      },
      { key: "newsletter_enabled", label: "Email confirmed subscribers when a post is published", hint: "Off by default: publishing never surprise-emails anyone. Needs an SMTP relay and a site address.", kind: "toggle" },
    ],
  },
  {
    id: "membership",
    title: "Membership",
    description: "Whether visitors can make their own account, and what they start out able to do.",
    keys: [
      { key: "registration_enabled", label: "Anyone can create an account", hint: "Off by default. Needs a mail relay and a site address — see the note below.", kind: "toggle" },
      { key: "registration_default_role", label: "Default role for new accounts", hint: "Offered: roles that hold none of manage users, options, plugins, themes or categories, edit others' content, or moderate comments — so never Administrator or Editor. A stranger who fills in the form gets at most an author's power over their own work.", kind: "role_select" },
    ],
  },
  {
    id: "delivery",
    title: "Delivery",
    description: "Caching in front of the site and the room uploads may take. Storage and CDN themselves are set in the environment; Site health shows what is configured.",
    step: "delivery",
    keys: [
      { key: "edge_cache_seconds", label: "Edge cache", hint: "How long a CDN may keep a page. 0 without a CDN; 60 is a good start with one. Up to 86400.", placeholder: "0", kind: "number", unit: "seconds" },
      { key: "media_storage_cap_mb", label: "Storage cap", hint: "Uploads stop at the cap and Site health warns near it. Empty for no cap.", placeholder: "none", kind: "number", unit: "MB" },
    ],
  },
  {
    id: "assistants",
    title: "Assistants",
    description: "Each feature uses the default model of its kind from the AI models page. Everything is off until you turn it on.",
    step: "assistants",
    keys: [
      { key: "ai_month_cap_usd", label: "Monthly cap", hint: "Every assistant refuses past it. Empty removes the cap.", placeholder: "20", kind: "money", unit: "USD" },
      { key: "ai_alt_text", label: "Write alt text for uploaded images", hint: "Vision model, on upload and on demand in the media library.", kind: "toggle" },
      {
        key: "ai_comment_screening",
        label: "Screen new comments",
        hint: "Moderation model. “Flag” records a verdict in the queue; “Spam” also moves flagged comments to spam.",
        kind: "select",
        options: [
          { value: "off", label: "Off" },
          { value: "flag", label: "Flag only" },
          { value: "spam", label: "Flag and move to spam" },
        ],
      },
      { key: "ai_autofill", label: "Fill empty excerpt and SEO description on publish", hint: "Text model. Fields with a value are never overwritten.", kind: "toggle" },
      { key: "ai_embeddings", label: "Embed posts on publish", hint: "Embedding model. Needed by semantic search and related posts.", kind: "toggle" },
      { key: "ai_semantic_search", label: "Semantic search", hint: "Re-orders full-text results by meaning. Needs embeddings.", kind: "toggle" },
      { key: "ai_related_posts", label: "Related posts block", hint: "Lets the theme's related-posts block show nearest posts. Needs embeddings.", kind: "toggle" },
      { key: "ai_images", label: "Image generation", hint: "Image model. Authors can generate images from the editor; capped at 50 a day site-wide.", kind: "toggle" },
      { key: "ai_transcription", label: "Transcription", hint: "Transcription model. Audio and video files can be transcribed from the media library and the editor.", kind: "toggle" },
      { key: "ai_read_aloud", label: "Read posts aloud", hint: "Speech model. Records an audio version on publish for the theme's read-aloud block.", kind: "toggle" },
    ],
  },
  {
    id: "updates",
    title: "Marketplace and updates",
    description: "Where plugins, themes and core releases come from. Both are built in and verified by keys compiled into this server; the operator can mirror or switch them off in vyasa.toml.",
    keys: [],
  },
  {
    id: "advanced",
    title: "Advanced",
    description: "Styles applied on top of your theme.",
    keys: [
      { key: "custom_css", label: "Custom CSS", hint: "Scoped to your site's content, so it can't affect this admin.", kind: "css" },
    ],
  },
];

const ALL_KEYS = SECTIONS.flatMap((s) => s.keys);

function sourceLine(what: "Official marketplace" | "Marketplace" | "Updates", info: SourceInfo | undefined): string {
  if (!info) return `${what} — checking…`;
  const noun = what === "Updates" ? "Updates" : info.state === "official" ? "Official marketplace" : "Marketplace";
  switch (info.state) {
    case "official":
      return noun === "Updates" ? "Updates — stable channel" : "Official marketplace — connected";
    case "mirror":
      return `${noun} — mirrored by your operator (${info.url})`;
    case "off":
      return `${noun} — turned off by your operator`;
  }
}

/** Read-only: these come from the binary and the server's own config. */
function SourcesPanel() {
  const sources = useQuery({ queryKey: ["registry-sources"], queryFn: () => api.registrySources() });
  return (
    <div className="2xl:col-span-2 space-y-1 text-sm" data-testid="sources-panel">
      <p>{sourceLine("Official marketplace", sources.data?.marketplace)}</p>
      <p>{sourceLine("Updates", sources.data?.updates)}</p>
      <p className="text-xs text-muted-foreground">Set in the server's vyasa.toml ([marketplace] / [updates]); see the deployment guide.</p>
    </div>
  );
}
const LABEL_OF = Object.fromEntries(ALL_KEYS.map((k) => [k.key, k.label]));

/**
 * The brand kit: durable context the assistants read before every
 * generation. Free text rather than pickers, because "quiet and
 * technical, no exclamation marks" carries more than any list of options.
 */
const BRAND_FIELDS: { key: string; label: string; placeholder: string }[] = [
  { key: "audience", label: "Who it is for", placeholder: "Engineers evaluating a CMS for a small team." },
  { key: "voice", label: "How it sounds", placeholder: "Plain and technical. No marketing gloss, no exclamation marks." },
  { key: "palette", label: "Colour", placeholder: "Warm neutrals with one rust accent. Never blue." },
  { key: "typography", label: "Type", placeholder: "A serif for headings against a neutral sans for body." },
  { key: "references", label: "Reference points", placeholder: "Stripe's docs for density; Linear for restraint." },
  { key: "avoid", label: "Never", placeholder: "Gradients, stock photography, hero images of laptops." },
];

/** Option value → form string. Lists become one item per line. */
function toForm(kind: Kind, v: unknown): string {
  if (v == null) return "";
  if (kind === "keys" && Array.isArray(v)) return v.map(String).join("\n");
  return String(v);
}

/** Form string → what the API expects for this key. */
function fromForm(kind: Kind, raw: string): unknown {
  switch (kind) {
    case "toggle": return raw === "true";
    case "number": return raw.trim() === "" ? null : Number(raw);
    case "money": return raw.trim() === "" ? null : Number(raw);
    case "keys": return raw.split(/\r?\n/).map((s) => s.trim()).filter(Boolean);
    default: return raw;
  }
}

export function SettingsPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const opts = useQuery({ queryKey: ["options"], queryFn: () => api.getOptions() });
  // The environment, for inline verification: SMTP presence gates the
  // newsletter; the same call the setup wizard makes.
  const checks = useQuery({ queryKey: ["setup-checks"], queryFn: () => api.setupChecks(), retry: false, staleTime: 60_000 });
  const relay = useQuery({ queryKey: ["mail-settings"], queryFn: () => api.mailSettings(), retry: false });
  const smtp = checks.data?.find((c) => c.name === "smtp");
  const hasRelay = smtp !== undefined && smtp.status === "ok";

  // Membership is security-sensitive (phase 97's rule): shown and editable
  // only for a full administrator, someone holding every capability, not
  // merely `manage_options`. Undetermined (still loading) reads as "no" —
  // a late reveal is a much smaller problem than a flash of a section that
  // then has to disappear.
  const caps = useCapabilities();
  const isFullAdmin = caps.data !== undefined && CAPABILITY_ORDER.every((c) => caps.can(c));
  const roles = useQuery({ queryKey: ["roles"], queryFn: () => api.listRoles(), enabled: isFullAdmin });
  // Whether registration can work is the server's call (a relay it can use
  // and a site address), not something to guess from other settings: the
  // same answer the sign-in page gets.
  const registrationInfo = useQuery({
    queryKey: ["registration-info"],
    queryFn: () => api.registrationInfo(),
    enabled: isFullAdmin,
    retry: false,
  });
  const visibleSections = SECTIONS.filter((s) => s.id !== "membership" || isFullAdmin);

  const [values, setValues] = React.useState<Record<string, string>>({});
  const [saved, setSaved] = React.useState<Record<string, string>>({});
  const [brand, setBrand] = React.useState<Record<string, string>>({});
  const [savedBrand, setSavedBrand] = React.useState<Record<string, string>>({});
  const [urlCheck, setUrlCheck] = React.useState<"idle" | "checking" | "ok" | "local" | "unreachable">("idle");

  const dirtyRef = React.useRef(false);
  React.useEffect(() => {
    if (opts.data === undefined || dirtyRef.current) return;
    const next: Record<string, string> = {};
    for (const k of ALL_KEYS) next[k.key] = toForm(k.kind, (opts.data as Record<string, unknown>)[k.key]);
    setValues(next);
    setSaved(next);
    const stored = (opts.data as Record<string, unknown>)["brand_kit"];
    const kit: Record<string, string> = {};
    for (const f of BRAND_FIELDS) {
      const v = stored !== null && typeof stored === "object" ? (stored as Record<string, unknown>)[f.key] : undefined;
      kit[f.key] = typeof v === "string" ? v : "";
    }
    setBrand(kit);
    setSavedBrand(kit);
  }, [opts.data]);

  const dirtyKeys = ALL_KEYS.filter((k) => values[k.key] !== saved[k.key]).map((k) => k.key);
  const brandDirty = BRAND_FIELDS.some((f) => brand[f.key] !== savedBrand[f.key]);
  const isDirty = dirtyKeys.length > 0 || brandDirty;
  dirtyRef.current = isDirty;
  useUnsavedChanges(() => dirtyRef.current);
  const changedLabels = [...dirtyKeys.map((k) => LABEL_OF[k] ?? k), ...(brandDirty ? ["Brand kit"] : [])];

  // Inline validation, mirroring the server's rules so the save bar can
  // say what is wrong before the request.
  const errors: Record<string, string | null> = {};
  const intIn = (key: string, lo: number, hi: number, allowEmpty: boolean) => {
    const v = (values[key] ?? "").trim();
    if (v === "") return allowEmpty ? null : "Enter a whole number.";
    if (!/^\d+$/.test(v)) return "Enter a whole number.";
    const n = Number(v);
    return n < lo || n > hi ? `Between ${lo} and ${hi}.` : null;
  };
  errors["posts_per_page"] = intIn("posts_per_page", 1, 100, true);
  errors["edge_cache_seconds"] = intIn("edge_cache_seconds", 0, 86_400, true);
  errors["media_storage_cap_mb"] = intIn("media_storage_cap_mb", 1, 10_000_000, true);
  {
    const v = (values["ai_month_cap_usd"] ?? "").trim();
    errors["ai_month_cap_usd"] = v !== "" && !(Number(v) >= 0) ? "A non-negative amount." : null;
  }
  {
    const v = (values["site_url"] ?? "").trim();
    errors["site_url"] = v !== "" && !/^https?:\/\/[^\s/]+/.test(v) ? "Must start with http:// or https:// and name a host." : urlCheck === "unreachable" ? "This address does not reach this server. Save anyway if DNS is still settling." : null;
  }
  const blocking = Object.entries(errors).some(([k, e]) => e !== null && k !== "site_url") || (errors["site_url"] !== null && urlCheck !== "unreachable");

  const save = useMutation({
    mutationFn: async () => {
      // A changed site address is checked first; an unreachable one is
      // reported and the save waits for a second click.
      if (dirtyKeys.includes("site_url") && urlCheck !== "unreachable") {
        setUrlCheck("checking");
        const r = await api.setupVerifyUrl((values["site_url"] ?? "").trim()).catch(() => ({ reachable: false, local: false }));
        if (!r.reachable && !r.local) {
          setUrlCheck("unreachable");
          throw new Error("The site address does not reach this server. Check it, or press Save again to keep it.");
        }
        setUrlCheck(r.reachable ? "ok" : "local");
      }
      const body: Record<string, unknown> = Object.fromEntries(
        dirtyKeys.map((key) => [key, fromForm(ALL_KEYS.find((k) => k.key === key)?.kind ?? "text", values[key] ?? "")]),
      );
      if (brandDirty) {
        body["brand_kit"] = Object.fromEntries(BRAND_FIELDS.map((f) => [f.key, (brand[f.key] ?? "").trim()]).filter(([, v]) => v !== ""));
      }
      await api.putOptions(body);
      return { values, brand };
    },
    onSuccess: (sent) => {
      void queryClient.invalidateQueries({ queryKey: ["options"] });
      void queryClient.invalidateQueries({ queryKey: ["registration-info"] });
      setSaved(sent.values);
      setSavedBrand(sent.brand);
      setUrlCheck("idle");
      notify.success("Settings saved", changedLabels.join(", "));
    },
    onError: (e) => notify.error("Nothing was saved", e),
  });

  const set = (key: string, value: string) => {
    if (key === "site_url") setUrlCheck("idle");
    setValues((prev) => ({ ...prev, [key]: value }));
  };

  const discard = async () => {
    const n = changedLabels.length;
    if (n > 1) {
      const ok = await confirm({ title: `Discard ${n} changes?`, description: changedLabels.join(", "), confirmLabel: "Discard" });
      if (!ok) return;
    }
    setValues(saved);
    setBrand(savedBrand);
    setUrlCheck("idle");
  };

  if (opts.isError) {
    return (
      <div className="space-y-4">
        <PageHeader title="Settings" />
        <ErrorNote title="Couldn't load your settings" error={opts.error} />
      </div>
    );
  }

  const control = (k: SettingKey) => {
    const id = `opt-${k.key}`;
    const v = values[k.key] ?? "";
    switch (k.kind) {
      case "media": return <MediaIdField id={id} value={v} onChange={(x) => set(k.key, x)} />;
      case "toggle": {
        const gated = k.key === "newsletter_enabled" && checks.data !== undefined && !hasRelay && relay.data?.source !== "options";
        return (
          <div className="space-y-1">
            <div className={cn(gated && "pointer-events-none opacity-60")}>
              <Switch id={id} label={k.label} checked={v === "true"} onChange={(x) => set(k.key, x ? "true" : "false")} />
            </div>
            {gated ? <p className="text-xs text-warning">No mail relay is configured, so the newsletter cannot send. Add one below.</p> : null}
          </div>
        );
      }
      case "select":
        return (
          <select id={id} value={v === "" ? (k.options?.[0]?.value ?? "") : v} onChange={(e) => set(k.key, e.target.value)} className="h-9 w-full rounded-md border bg-background px-2 text-sm">
            {(k.options ?? []).map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
          </select>
        );
      case "css": return <MonacoPane language="css" value={v} onChange={(x) => set(k.key, x)} height={220} ariaLabel={k.label} />;
      case "timezone": return <TimezoneField id={id} value={v} onChange={(x) => set(k.key, x)} />;
      case "date_format": return <DateFormatField id={id} value={v} onChange={(x) => set(k.key, x)} />;
      case "permalink": return <PermalinkField id={id} value={v} onChange={(x) => set(k.key, x)} />;
      case "keys": return <KeysField id={id} value={v} onChange={(x) => set(k.key, x)} />;
      case "role_select": return <RoleSelectField id={id} value={v} onChange={(x) => set(k.key, x)} roles={roles.data ?? []} />;
      case "number":
      case "money":
        return (
          <div className="flex items-center gap-2">
            <Input id={id} inputMode={k.kind === "money" ? "decimal" : "numeric"} value={v} placeholder={k.placeholder} onChange={(e) => set(k.key, e.target.value)} className="tabular-nums" />
            {k.unit ? <span className="text-sm text-muted-foreground">{k.unit}</span> : null}
          </div>
        );
      default:
        return (
          <div className="flex items-center gap-2">
            <Input id={id} value={v} placeholder={k.placeholder} onChange={(e) => set(k.key, e.target.value)} className={k.kind === "url" ? "font-mono text-sm" : undefined} />
            {k.key === "site_url" && urlCheck === "checking" ? <span className="text-xs text-muted-foreground">checking…</span> : null}
            {k.key === "site_url" && urlCheck === "ok" ? <span className="text-xs text-success">reaches this server</span> : null}
            {k.key === "site_url" && urlCheck === "local" ? <span className="text-xs text-muted-foreground">local address, not tested</span> : null}
          </div>
        );
    }
  };

  return (
    <div className="pb-24">
      <PageHeader
        title="Settings"
        description="Site-wide options that apply to every page you publish."
        actions={
          <a href="/admin/setup?again=1" className={buttonVariants({ variant: "outline", size: "sm" })}>
            Run setup again
          </a>
        }
      />

      <div className="mt-4 grid gap-6 xl:grid-cols-[14rem_minmax(0,1fr)]">
        {/* Section rail: the page is long; this is how you get to Comments
            without scrolling past everything. */}
        <nav aria-label="Settings sections" className="hidden xl:block">
          <ol className="sticky top-4 space-y-0.5 text-sm">
            {visibleSections.map((s) => (
              <li key={s.id}>
                <a href={`#settings-${s.id}`} className="block rounded-md px-3 py-1.5 text-muted-foreground hover:bg-accent hover:text-foreground">{s.title}</a>
              </li>
            ))}
          </ol>
        </nav>

        <div className="min-w-0 space-y-5">
          {opts.isPending ? (
            <div className="space-y-4">{[0, 1, 2].map((i) => <Skeleton key={i} className="h-40 w-full rounded-lg" />)}</div>
          ) : (
            visibleSections.map((section) => (
              <section key={section.id} id={`settings-${section.id}`} className="scroll-mt-4">
                <Panel title={section.title} description={section.description}>
                  <div className="grid gap-x-8 gap-y-4 2xl:grid-cols-2">
                    {section.keys.map((k) => (
                      <div key={k.key} className={cn(k.kind === "css" ? "2xl:col-span-2" : "max-w-[70ch]")}>
                        <Field label={k.label} htmlFor={`opt-${k.key}`} hint={k.hint} error={errors[k.key] ?? null}>
                          <div className={WIDTH[k.kind]}>{control(k)}</div>
                        </Field>
                      </div>
                    ))}
                    {section.id === "updates" ? <SourcesPanel /> : null}
                    {section.id === "advanced" ? (
                      <div className="2xl:col-span-2 border-t pt-4">
                        <ExportImportPanel />
                      </div>
                    ) : null}
                    {section.id === "membership" ? (
                      <div className="2xl:col-span-2 border-t pt-4" data-testid="membership-mail-note">
                        {registrationInfo.data?.enabled && !registrationInfo.data.available ? (
                          <p className="text-xs text-warning">
                            Registration is on, but every attempt is refused: this server can't send the confirmation email. It needs a mail relay (below) and a site address (above, under Site identity); Site health says which is missing.
                          </p>
                        ) : registrationInfo.data?.available ? null : (
                          <p className="text-xs text-muted-foreground">Registration needs both a mail relay and a site address configured — without them every attempt is refused.</p>
                        )}
                      </div>
                    ) : null}
                    {section.id === "mail" ? (
                      <div className="2xl:col-span-2 border-t pt-4">
                        <h3 className="text-sm font-medium">Mail relay</h3>
                        <p className="mb-3 mt-0.5 max-w-[70ch] text-xs text-muted-foreground">Where password resets, notifications and the newsletter go out. Saves on its own, separately from the fields above.</p>
                        <MailRelayPanel />
                      </div>
                    ) : null}
                    {section.id === "assistants" ? (
                      <div className="2xl:col-span-2" data-testid="brand-kit">
                        <h3 className="text-sm font-medium">Brand kit</h3>
                        <p className="mb-3 mt-0.5 max-w-[70ch] text-xs text-muted-foreground">What the assistants read before every generation. Without it they have only your site's title to go on, and every site in a category has a similar title.</p>
                        <div className="grid gap-4 lg:grid-cols-2 2xl:grid-cols-3">
                          {BRAND_FIELDS.map((f) => (
                            <Field key={f.key} label={f.label} htmlFor={`brand-${f.key}`}>
                              <textarea
                                id={`brand-${f.key}`}
                                rows={2}
                                value={brand[f.key] ?? ""}
                                placeholder={f.placeholder}
                                maxLength={600}
                                onChange={(e) => setBrand((prev) => ({ ...prev, [f.key]: e.target.value }))}
                                className="w-full max-w-[70ch] resize-y rounded-md border border-input bg-background p-2 text-sm placeholder:text-muted-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
                              />
                            </Field>
                          ))}
                        </div>
                      </div>
                    ) : null}
                  </div>
                  {section.id === "assistants" ? <AiBackfill options={opts.data ?? {}} /> : null}
                  {section.step ? (
                    <p className="mt-4 border-t pt-3 text-xs text-muted-foreground">
                      Prefer the guided version? <a href={`/admin/setup?again=1&step=${section.step}`} className="text-primary underline-offset-2 hover:underline">Rerun the {section.title.toLowerCase()} step</a> of setup.
                    </p>
                  ) : null}
                </Panel>
              </section>
            ))
          )}
        </div>
      </div>

      {/* Save bar appears only when something changed, names what, and
          stays reachable above the phone's home indicator. */}
      {isDirty ? (
        <div className="fixed inset-x-0 bottom-0 z-30 border-t bg-card/95 px-3 py-3 pb-[calc(0.75rem+env(safe-area-inset-bottom))] backdrop-blur-sm sm:px-6" data-testid="save-bar">
          <div className="mx-auto flex items-center gap-3">
            <span className="min-w-0 flex-1 truncate text-sm text-muted-foreground" title={changedLabels.join(", ")}>
              <b className="font-medium text-foreground">{changedLabels.length} unsaved {changedLabels.length === 1 ? "change" : "changes"}:</b> {changedLabels.join(", ")}
            </span>
            <Button variant="outline" size="sm" onClick={() => void discard()} disabled={save.isPending}>Discard</Button>
            <Button size="sm" onClick={() => save.mutate()} disabled={save.isPending || blocking}>
              {save.isPending ? "Saving…" : urlCheck === "unreachable" ? "Save anyway" : "Save changes"}
            </Button>
          </div>
        </div>
      ) : null}
    </div>
  );
}


function AiBackfill({ options }: { options: Record<string, unknown> }) {
  const [feature, setFeature] = React.useState<"embeddings" | "autofill" | "screen">("embeddings");
  const [after, setAfter] = React.useState<string | undefined>();
  const [message, setMessage] = React.useState("");
  const enabled = feature === "screen" ? options.ai_comment_screening === "flag" || options.ai_comment_screening === "spam" : options[feature === "embeddings" ? "ai_embeddings" : "ai_autofill"] === true;
  const run = useMutation({
    mutationFn: () => api.backfillAi(feature, after),
    onSuccess: (r) => { setAfter(r.next_after_id ?? undefined); setMessage(`${r.queued} jobs queued.${r.next_after_id ? " More items are available." : " End of this pass."} Progress and failures appear in Site health.`); },
    onError: (e) => notify.error("Couldn't queue existing content", e),
  });
  return <div className="mt-4 space-y-3 rounded-md border p-3">
    <p className="text-sm font-medium">Process existing content</p>
    <p className="text-xs text-muted-foreground">Uses your configured models and budget. Each click queues at most 100 items with missing results. Save and enable the feature first. Existing field values are kept; screening follows your moderation setting.</p>
    <select aria-label="AI operation" value={feature} disabled={run.isPending} onChange={(e) => { setFeature(e.target.value as typeof feature); setAfter(undefined); setMessage(""); }} className="h-9 rounded-md border bg-background px-2 text-sm">
      <option value="embeddings">Generate missing embeddings</option><option value="autofill">Fill empty post fields</option><option value="screen">Screen comments without a verdict</option>
    </select>
    <Button size="sm" variant="outline" disabled={!enabled || run.isPending} onClick={() => run.mutate()}>{run.isPending ? "Queueing…" : after ? "Process next batch" : "Process up to 100 items"}</Button>
    {message ? <p role="status" className="text-xs text-muted-foreground">{message}</p> : null}
  </div>;
}
