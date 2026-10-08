import type { components } from "./schema";
import { parseJsonPreservingIds } from "./json-bigint";

/** Generated schema types re-exported for convenience. */
/** Where the marketplace or update channel points, as the server reports it. */
export type SourceInfo = { state: "official" | "mirror" | "off"; url: string };

export type UserResponse = IdAsString<components["schemas"]["UserResponse"]>;
/**
 * The account an administrator's "Confirm" left, and `link_sent`: whether
 * the set-password link went out (absent when the account was confirmed
 * already and nothing was due).
 */
export type ConfirmResponse = IdAsString<components["schemas"]["ConfirmResponse"]>;
/**
 * Ids are 64-bit on the wire and arrive as strings (see ./json-bigint.ts), so
 * the generated `number` typing would be a lie.
 *
 * Every field naming a row — `id`, `post_id`, `parent_id`, … — is remapped,
 * because comparing or stringifying any of them as a number reintroduces the
 * same precision bug.
 */
type IdField =
  | "id"
  | "post_id"
  | "parent_id"
  | "author_id"
  | "owner_id"
  | "term_id"
  | "menu_id"
  | "webhook_id";

type IdAsString<T> = Omit<T, IdField & keyof T> & {
  [K in IdField & keyof T]: null extends T[K] ? string | null : string;
};

export type PostResponse = IdAsString<components["schemas"]["PostResponse"]>;

/** Custom field values, keyed by field key (media and entry ids as strings). */
export type FieldValues = components["schemas"]["PostResponse"]["fields"];

/** A content type as `GET /content-types` lists it. */
export type ContentTypeInfo = Omit<components["schemas"]["ContentType"], "owner"> & {
  /** Only `admin` types can be relabelled or deleted. */
  owner: "builtin" | "plugin" | "admin";
};

/** An administrator's content type, as create and relabel return it. */
export type AdminContentType = components["schemas"]["AdminType"];

/** The nine kinds a custom field can be. */
export type FieldKind =
  | "text"
  | "textarea"
  | "number"
  | "boolean"
  | "date"
  | "choice"
  | "url"
  | "media"
  | "entry";

/** A kind's options; which keys apply depends on the kind. */
export interface FieldOptions {
  /** text, textarea */
  max_length?: number;
  /** number */
  min?: number;
  max?: number;
  step?: number;
  /** choice */
  choices?: string[];
  multiple?: boolean;
  /** entry: the one type it may point at (any type when absent). */
  entry_type?: string;
}

/**
 * A field definition (`/content-types/{slug}/fields`). The generated type
 * leaves `kind` a string and `options` any JSON; both are narrowed here.
 */
export type ContentField = Omit<components["schemas"]["ContentField"], "kind" | "options"> & {
  kind: FieldKind;
  options: FieldOptions;
};

/** Body of `POST /content-types/{slug}/fields`. */
export type ContentFieldInput = Omit<components["schemas"]["ContentFieldInput"], "kind" | "options"> & {
  kind: FieldKind;
  options?: FieldOptions;
};

/** Body of `PUT /content-types/{slug}/fields/{key}`: what to change. */
export type ContentFieldChanges = Omit<components["schemas"]["ContentFieldChanges"], "kind" | "options" | "key"> & {
  kind?: FieldKind;
  options?: FieldOptions;
};

/** Values a deleted field left behind: entries still store them. */
export type OrphanValues = components["schemas"]["ContentFieldOrphan"];

/** A revision; `fields` is null for one older than fields. */
export type PostRevision = IdAsString<components["schemas"]["PostRevisionResponse"]>;
export type PaginatedPosts = Omit<
  components["schemas"]["PaginatedPosts"],
  "items"
> & { items: PostResponse[] };
export type CommentResponse = IdAsString<components["schemas"]["CommentResponse"]>;
export type MediaResponse = IdAsString<components["schemas"]["MediaResponse"]>;
export type ApiErrorBody = components["schemas"]["ApiErrorBody"];
/** A role as `GET /roles` returns it — built-in or custom. */
export type RoleResponse = components["schemas"]["RoleResponse"];

/** `GET /auth/registration`: whether the site takes new accounts, and the server's password rule. */
export interface RegistrationInfo {
  /** `registration_enabled`, the owner's switch. */
  enabled: boolean;
  /** Enabled *and* able to work now (a mail relay and a site address): what decides whether to offer the form. */
  available: boolean;
  password_min_length: number;
}

/** Plugin summary returned by GET /plugins (phase 35). */
/** One site-health diagnostic, mirroring `vyasa_core::health::Check`. */
export interface HealthCheck {
  name: string;
  status: "ok" | "warn" | "fail";
  detail: string;
  /** core, content, delivery, assistants, operations. */
  group?: string;
  /** The remedy: a page to open, or an operation the page can run. */
  action?: { label: string; href?: string; op?: string };
}

/** A webhook as the list shows it. */
export interface WebhookSummary {
  id: string;
  url: string;
  events: string[];
  enabled: boolean;
  last_delivery: { status: "success" | "failed" | "dead"; at: string } | null;
}

/** What create returns: the summary plus the secret, shown once. */
export interface WebhookCreated extends Omit<WebhookSummary, "last_delivery"> {
  secret: string;
  signature_header: string;
}

/** One delivery attempt. */
export interface WebhookDelivery {
  id: string;
  event: string;
  status: "success" | "failed" | "dead";
  response_code: number | null;
  attempts: number;
  at: string;
  payload: string | null;
  response_body: string | null;
  error: string | null;
}

/** Everything the site holds about one email address. */
export interface PersonalData {
  email: string;
  account: Record<string, unknown> | null;
  comments: Record<string, unknown>[];
  submissions: Record<string, unknown>[];
  subscriptions: Record<string, unknown>[];
  posts: { id: string; title: string; status: string }[];
  exported_at: string;
}
/** One recorded administrative action. */
export interface AuditEntry {
  id: string;
  actor_id: string | null;
  actor_name: string;
  action: string;
  target: string;
  detail: Record<string, unknown>;
  ip: string;
  created_at: string;
}

/** One field of a designed form. */
export interface FormField {
  key: string;
  label: string;
  kind: "text" | "email" | "textarea" | "number" | "select" | "checkbox";
  required: boolean;
  options: string[];
}
export interface FormDef {
  id: string;
  name: string;
  slug: string;
  fields: FormField[];
  notify_email: string;
  success_message: string;
  enabled: boolean;
  unread: number;
  created_at: string;
  updated_at: string;
}
export interface FormInput {
  name: string;
  fields: FormField[];
  notify_email: string;
  success_message: string;
  enabled: boolean;
}
export interface FormSubmission {
  id: string;
  name: string;
  email: string;
  message: string;
  path: string;
  data: Record<string, string>;
  read_at: string | null;
  created_at: string;
}

/** A saved arrangement of blocks. */
export interface Pattern {
  id: string;
  name: string;
  slug: string;
  category: string;
  synced: boolean;
  blocks: unknown[];
  created_at: string;
  updated_at: string;
}
export interface PatternInput {
  name: string;
  category: string;
  synced: boolean;
  blocks: unknown[];
}

/** Where media bytes go, as the admin panel sees it: never the secret. */
export interface StorageSettings {
  provider: "local" | "s3";
  bucket: string;
  region: string;
  endpoint: string;
  path_style: boolean;
  access_key_id_hint: string;
  has_secret: boolean;
  keys_unreadable: boolean;
  source: "options" | "environment" | "none";
  encrypted: boolean;
  counts: { local: number; s3: number };
  ephemeral_disk: boolean;
  migration: MigrationProgress | null;
}

/** How far "Move existing files" got. */
export interface MigrationProgress {
  state: "running" | "done" | "failed";
  total: number;
  done: number;
  failed: number;
  to: "local" | "s3";
  started_at: string;
  finished_at: string | null;
  last_error: string | null;
}

/** What the storage panel sends; omitted keys keep the stored ones. */
export interface StorageInput {
  provider: "local" | "s3";
  bucket?: string;
  region?: string;
  endpoint?: string;
  path_style?: boolean;
  access_key_id?: string;
  secret_access_key?: string;
  forget_existing?: boolean;
}

/** The mail relay as the admin panel sees it: never the password. */
export interface MailSettings {
  host: string;
  port: number;
  username: string;
  from: string;
  has_password: boolean;
  source: "options" | "environment" | "none";
  encrypted: boolean;
}

/** The whole site-health report. */
export interface SiteHealthReport {
  status: "ok" | "warn" | "fail";
  checks: HealthCheck[];
  checked_at?: string;
  warnings?: number;
  failures?: number;
}

export interface PluginSummary {
  id: string;
  name: string;
  version: string;
  enabled: boolean;
  status: string;
  status_reason?: string;
  capabilities: string[];
  description?: string;
  author?: string;
  homepage?: string;
  license?: string;
  versions?: string[];
  audit?: Record<string, number>;
}

/** A package parsed but not installed. */
export interface PluginInspection {
  name: string;
  version: string;
  description: string;
  author: string;
  homepage: string;
  license: string;
  capabilities: string[];
  trusted: boolean;
  signature_prefix: string;
  wasm_bytes: number;
  installed_version: string | null;
  new_capabilities: string[];
}

/** One audit row: a denied capability, a fetch, or a quota hit. */
export interface PluginAuditRow {
  ts: string;
  kind: "deny" | "fetch" | "quota";
  capability: string;
  detail: string;
}

/** One field of a plugin's declared settings form. */
export interface PluginField {
  key: string;
  label: string;
  kind: "text" | "textarea" | "number" | "boolean" | "select";
  help: string | null;
  options: string[];
  default: unknown;
}

/** A settings form one plugin declared. */
export interface PluginForm {
  pluginId: string;
  pluginName: string;
  title: string;
  description: string | null;
  fields: PluginField[];
}

/** Everything installed plugins add to the running site. */
export interface PluginSurface {
  blocks: {
    kind: string;
    title: string;
    icon: string | null;
    pluginId: string;
    pluginName: string;
  }[];
  routes: { method: string; path: string; pluginId: string }[];
  forms: PluginForm[];
  postTypes: {
    slug: string;
    singular: string;
    plural: string;
    public: boolean;
    hasArchive: boolean;
    pluginId: string;
  }[];
  taxonomies: {
    slug: string;
    singular: string;
    plural: string;
    hierarchical: boolean;
    public: boolean;
    pluginId: string;
  }[];
  tasks: {
    pluginId: string;
    name: string;
    everySeconds: number;
    lastRunAt: string | null;
    lastStatus: string;
    lastError: string | null;
  }[];
  /** Every filter point a plugin may hook, for reference. */
  filterPoints: string[];
  /** Every event name a plugin may handle. */
  eventNames: string[];
}

/** Response of a successful plugin install. */
export interface PluginInstallResponse {
  id: string;
  name: string;
  version: string;
  /** `degraded` when an upgrade's declarations clash with a content type. */
  status?: string;
  enabled?: boolean;
}

// The generated `operations` map collides on duplicate operation names
// (`list` is used by both /posts and /media), so query-param shapes are
// mirrored here from the OpenAPI document; response bodies stay generated.
/** A comment screening verdict, as stored on the comment. */
export interface CommentModeration {
  flagged: boolean;
  top: { category: string; score: number }[];
  model: string;
  action: "none" | "spam" | string;
  at: string;
}

/** One provider on the Models page. Never carries the key itself. */
export interface AiProvider {
  provider: string;
  label: string;
  configured: boolean;
  key_source: "stored" | "environment" | "none";
  key_hint: string;
  key_sealed: boolean;
  base_url: string;
  default_base_url: string;
  enabled: boolean;
  supports: string[];
  needs_key: boolean;
  needs_base_url: boolean;
  recommended: Record<string, { model: string; label: string; input_cost_per_mtok: number | null; output_cost_per_mtok: number | null }>;
}

/** One registered model. */
export interface AiModel {
  id: string;
  provider: string;
  model: string;
  kind: string;
  label: string;
  enabled: boolean;
  is_default: boolean;
  settings: Record<string, unknown>;
  input_cost_per_mtok: number | null;
  output_cost_per_mtok: number | null;
  last_probe_ok: boolean | null;
  last_probe_detail: string;
  last_probe_at: string | null;
  created_at: string;
  updated_at: string;
  sort_order?: number;
  breaker_open?: boolean;
  provider_enabled?: boolean;
  month_calls?: number;
  month_cost_usd?: number;
}

export interface AiKind {
  kind: string;
  label: string;
  used_by: string;
}

export interface AiRegistry {
  encrypting_keys: boolean;
  providers: AiProvider[];
  models: AiModel[];
  kinds: AiKind[];
  spend?: { total_usd: number; cap_usd: number | null; by_purpose: Record<string, number> };
}

export interface AiCatalogEntry {
  id: string;
  name: string;
  kinds: string[];
}

/** One marketplace listing, with what this site already has. */
export interface RegistryEntry {
  kind: "plugin" | "theme";
  name: string;
  title: string;
  summary: string;
  author: string;
  homepage: string | null;
  versions: {
    version: string;
    capabilities: string[];
    released_at: string | null;
  }[];
  installed_version: string | null;
  update_available: boolean;
  new_capabilities: string[];
}

/** How a running upgrade is going. */
export interface UpdateProgress {
  target: string;
  stage:
    | "preflight"
    | "downloading"
    | "verifying"
    | "backing_up"
    | "swapping"
    | "migrating"
    | "restarting"
    | "done"
    | "failed"
    | "rolled_back";
  detail: string;
  backup_path: string | null;
  started_at: string;
}

export interface ListPostsQuery {
  status?: string | null;
  type?: string | null;
  author_id?: string | number | null;
  term_id?: number | null;
  search?: string | null;
  page?: number | null;
  per_page?: number | null;
}

export interface ListCommentsQuery {
  status?: string | null;
  limit?: number | null;
  offset?: number | null;
}
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, body: Partial<ApiErrorBody> | null) {
    super(body?.message ?? `request failed with ${status}`);
    this.name = "ApiError";
    this.status = status;
    this.code = body?.code ?? "unknown";
  }
}

/**
 * A readable error for a failed render request.
 *
 * Render endpoints answer with HTML, so a failure body may be a whole
 * error page; showing it verbatim puts raw markup in the preview banner.
 * Keep the status and a short, tag-free snippet (or the API's `message`
 * when the body is the usual JSON error).
 */
export function previewFailure(status: number, text: string): ApiError {
  let message: string | undefined;
  let code: string | undefined;
  try {
    const parsed = JSON.parse(text) as Partial<ApiErrorBody> | null;
    if (parsed !== null && typeof parsed === "object" && typeof parsed.message === "string") {
      message = parsed.message;
      code = typeof parsed.code === "string" ? parsed.code : undefined;
    }
  } catch {
    // Not JSON.
  }
  if (message === undefined) {
    const plain = text
      .replace(/<(script|style)\b[\s\S]*?<\/\1\s*>/gi, " ")
      .replace(/<[^>]*>/g, " ")
      .replace(/&nbsp;/g, " ")
      .replace(/&lt;/g, "<")
      .replace(/&gt;/g, ">")
      .replace(/&amp;/g, "&")
      .replace(/\s+/g, " ")
      .trim();
    message = plain.length > 160 ? `${plain.slice(0, 160)}…` : plain;
  }
  const full = message === "" ? `Preview failed (${status})` : `Preview failed (${status}): ${message}`;
  return new ApiError(status, { message: full, ...(code === undefined ? {} : { code }) });
}

const BASE = "";

async function request<T>(
  path: string,
  init?: RequestInit,
): Promise<T> {
  const response = await fetch(`${BASE}${path}`, {
    credentials: "same-origin",
    headers:
      init?.body instanceof FormData
        ? undefined
        : init?.body
          ? { "content-type": "application/json" }
          : undefined,
    ...init,
  });
  if (!response.ok) {
    let body: Partial<ApiErrorBody> | null = null;
    try {
      body = (await response.json()) as Partial<ApiErrorBody>;
    } catch {
      // non-JSON error body
    }
    throw new ApiError(response.status, body);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  // Not `response.json()`: 64-bit ids lose precision through the built-in
  // parser. See ./json-bigint.ts.
  return parseJsonPreservingIds<T>(await response.text());
}

/** Like `request`, but also returns the `x-total-count` header. */
async function requestWithTotal<T extends unknown[]>(path: string): Promise<{ body: T; total: number }> {
  const response = await fetch(`${BASE}${path}`, { credentials: "same-origin" });
  if (!response.ok) {
    let body: Partial<ApiErrorBody> | null = null;
    try {
      body = (await response.json()) as Partial<ApiErrorBody>;
    } catch {
      // non-JSON error body
    }
    throw new ApiError(response.status, body);
  }
  const body = parseJsonPreservingIds<T>(await response.text());
  const total = Number(response.headers.get("x-total-count") ?? body.length);
  return { body, total: Number.isFinite(total) ? total : body.length };
}

function queryString(params: object): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== "") {
      search.set(key, String(value));
    }
  }
  const s = search.toString();
  return s ? `?${s}` : "";
}

/** Typed API surface over the generated schema. */
export const api = {
  myCaps(): Promise<string[]> {
    return request("/api/v1/auth/me/caps");
  },

  me(): Promise<UserResponse> {
    return request("/api/v1/auth/me");
  },

  /** Signs in with a code after the password step answered 202. */
  loginMfa(challenge: string, code: string): Promise<UserResponse> {
    return request("/api/v1/auth/mfa", { method: "POST", body: JSON.stringify({ challenge, code }) });
  },
  mfaStatus(): Promise<{ enabled: boolean; pending: boolean; recovery_codes_left: number }> {
    return request("/api/v1/auth/mfa/status");
  },
  mfaSetup(): Promise<{ otpauth_url: string; qr_svg: string; secret: string }> {
    return request("/api/v1/auth/mfa/setup", { method: "POST" });
  },
  mfaConfirm(code: string): Promise<{ codes: string[] }> {
    return request("/api/v1/auth/mfa/confirm", { method: "POST", body: JSON.stringify({ code }) });
  },
  mfaDisable(code: string): Promise<void> {
    return request("/api/v1/auth/mfa/disable", { method: "POST", body: JSON.stringify({ code }) });
  },
  resetUserMfa(id: string): Promise<void> {
    return request(`/api/v1/users/${id}/mfa`, { method: "DELETE" });
  },

  login(email: string, password: string): Promise<UserResponse> {
    return request("/api/v1/auth/login", {
      method: "POST",
      body: JSON.stringify({ email, password }),
    });
  },

  logout(): Promise<void> {
    return request("/api/v1/auth/logout", { method: "POST" });
  },

  listPosts(query: ListPostsQuery): Promise<PaginatedPosts> {
    return request(`/api/v1/posts${queryString(query)}`);
  },

  getPost(id: string): Promise<PostResponse> {
    return request(`/api/v1/posts/${id}`);
  },

  createPost(body: {
    title: string;
    content: unknown;
    status?: string | null;
    slug?: string | null;
    type?: string | null;
    excerpt?: string | null;
    password?: string | null;
    term_ids?: string[] | null;
    scheduled_for?: string | null;
    parent_id?: string | null;
    /** Section tree for a page that composes itself. */
    layout?: unknown;
        sticky?: boolean;
          lang?: string;
      translation_of?: string;
      /** Custom field values; checked by the server against the type's fields. */
      fields?: FieldValues;
    }): Promise<PostResponse> {
    return request("/api/v1/posts", {
      method: "POST",
      body: JSON.stringify(body),
    });
  },

  updatePost(
    id: string,
    body: {
      meta?: Record<string, unknown>;
      title?: string | null;
      content?: unknown;
      status?: string | null;
      slug?: string | null;
      excerpt?: string | null;
      password?: string | null;
      term_ids?: string[] | null;
      scheduled_for?: string | null;
      /** Section tree; `[]` clears it and restores the theme template. */
      layout?: unknown;
      /** Apply only if the post is unchanged since this moment (409 otherwise). */
      expected_updated_at?: string;
          sticky?: boolean;
          lang?: string;
      translation_of?: string;
      /**
       * Replaces the whole set of field values (`{}` clears them); left out,
       * the stored values are kept. Never put values in `meta`: the server
       * ignores `meta.fields`.
       */
      fields?: FieldValues;
    },
  ): Promise<PostResponse> {
    return request(`/api/v1/posts/${id}`, {
      method: "PUT",
      body: JSON.stringify(body),
    });
  },

  /**
   * Asks the designer for a page's sections. Nothing is saved: the tree
   * comes back for the author to keep or discard, then save as usual.
   */
  composePage(
    id: string,
    body: { message: string; content?: unknown; sections?: unknown },
  ): Promise<{
    reply: string;
    sections: unknown;
    warnings: { level: string; path: string; message: string }[];
    steps?: { thought: string; tool: string; observation: string }[];
  }> {
    return request(`/api/v1/posts/${id}/compose`, {
      method: "POST",
      body: JSON.stringify(body),
    });
  },

  /**
   * The page as it would look if saved. Returns HTML, not JSON — the
   * editor drops it straight into a preview frame.
   */
  async renderPage(
    id: string,
    body: { content?: unknown; sections?: unknown },
  ): Promise<string> {
    const r = await fetch(`/api/v1/posts/${id}/render`, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const text = await r.text();
    if (!r.ok) throw previewFailure(r.status, text);
    return text;
  },

  trashPost(id: string, force?: boolean): Promise<unknown> {
    return request(
      `/api/v1/posts/${id}${force ? "?force=true" : ""}`,
      { method: "DELETE" },
    );
  },

  duplicatePost(id: string): Promise<PostResponse> {
    return request(`/api/v1/posts/${id}/duplicate`, { method: "POST" });
  },

  batchPosts(ids: string[], action: "publish" | "draft" | "trash" | "restore" | "pin" | "unpin" | "add_terms", termIds: string[] = []): Promise<{ done: number; failed: { id: string; message: string }[] }> {
    return request("/api/v1/posts/batch", { method: "POST", body: JSON.stringify({ ids: ids.map(Number), action, term_ids: termIds.map(Number) }) });
  },

  restoreMedia(id: string): Promise<MediaResponse> {
    return request(`/api/v1/media/${id}/restore`, { method: "POST" });
  },

  emptyMediaTrash(): Promise<{ purged: number }> {
    return request("/api/v1/media/trash/empty", { method: "POST" });
  },

  restorePost(id: string): Promise<PostResponse> {
    return request(`/api/v1/posts/${id}/restore`, { method: "POST" });
  },

  previewToken(
    id: string,
  ): Promise<{ url: string; expires_at: string }> {
    return request(`/api/v1/posts/${id}/preview-token`, { method: "POST" });
  },

  autosave(
    id: string,
    body: { title: string; content: unknown; excerpt?: string | null; fields?: FieldValues },
  ): Promise<unknown> {
    return request(`/api/v1/posts/${id}/autosave`, {
      method: "PUT",
      body: JSON.stringify(body),
    });
  },

  /**
   * Saves a working copy without touching the post: what is live stays
   * live. "Save" on a published post; "Update" is `updatePost`.
   */
  saveRevision(
    id: string,
    body: { title: string; content: unknown; fields?: FieldValues },
  ): Promise<PostRevision> {
    return request(`/api/v1/posts/${id}/revisions`, {
      method: "POST",
      body: JSON.stringify(body),
    });
  },

  listRevisions(id: string): Promise<PostRevision[]> {
    return request(`/api/v1/posts/${id}/revisions`);
  },

  restoreRevision(postId: string, rid: string): Promise<PostResponse> {
    return request(`/api/v1/posts/${postId}/revisions/${rid}/restore`, {
      method: "POST",
    });
  },

  /** The terms on one post, so the editor starts from what is really set. */
  postTerms(postId: string): Promise<{ id: string; taxonomy: string; name: string; slug: string }[]> {
    return request(`/api/v1/posts/${postId}/terms`);
  },

  listTerms(params?: {
    taxonomy?: string;
    post_counts?: boolean;
  }): Promise<{ id: string; taxonomy: string; name: string; slug: string; parent_id?: string | null; post_count?: number | null }[]> {
    return request(`/api/v1/terms${queryString(params ?? {})}`);
  },

  createTerm(body: { taxonomy: string; name: string; slug?: string | null; parent_id?: string | null }): Promise<{ id: string; taxonomy: string; name: string; slug: string }> {
    return request("/api/v1/terms", { method: "POST", body: JSON.stringify(body) });
  },

  updateTerm(id: string, body: { name?: string | null; slug?: string | null; parent_id?: string | null }): Promise<unknown> {
    return request(`/api/v1/terms/${id}`, { method: "PUT", body: JSON.stringify(body) });
  },

  deleteTerm(id: string): Promise<void> {
    return request(`/api/v1/terms/${id}`, { method: "DELETE" });
  },

  mergeTerms(from: string, into: string): Promise<void> {
    return request("/api/v1/terms/merge", { method: "POST", body: JSON.stringify({ from, into }) });
  },

  listCommentsByStatus(
    query: ListCommentsQuery,
  ): Promise<(CommentResponse & { moderation?: CommentModeration | null })[]> {
    return request(`/api/v1/comments${queryString(query)}`);
  },

  async listCommentsPage(query: ListCommentsQuery): Promise<{ items: CommentResponse[]; total: number }> {
    const { body, total } = await requestWithTotal<CommentResponse[]>(`/api/v1/comments${queryString(query)}`);
    return { items: body, total };
  },

  moderateComment(id: string, action: "approve" | "spam" | "trash" | "restore"): Promise<CommentResponse> {
    return request(`/api/v1/comments/${id}/${action}`, { method: "POST" });
  },

  getMedia(id: string): Promise<MediaResponse> {
    return request(`/api/v1/media/${id}`);
  },

  listMedia(limit: number, offset: number): Promise<MediaResponse[]> {
    return request(`/api/v1/media${queryString({ limit, offset })}`);
  },

  /** A filtered page of the library with the total the filter matches. */
  async listMediaPage(query: {
    limit: number;
    offset: number;
    search?: string;
    kind?: string;
    sort?: string;
    owner_id?: string;
    trashed?: boolean;
  }): Promise<{ items: MediaResponse[]; total: number }> {
    const response = await fetch(`${BASE}/api/v1/media${queryString(query)}`, { credentials: "same-origin" });
    if (!response.ok) {
      let body: Partial<ApiErrorBody> | null = null;
      try {
        body = (await response.json()) as Partial<ApiErrorBody>;
      } catch {
        // non-JSON error body
      }
      throw new ApiError(response.status, body);
    }
    const items = parseJsonPreservingIds<MediaResponse[]>(await response.text());
    const total = Number(response.headers.get("x-total-count") ?? items.length);
    return { items, total: Number.isFinite(total) ? total : items.length };
  },

  mediaStats(): Promise<{ count: number; bytes: number; missing_alt: number; cap_bytes: number | null }> {
    return request("/api/v1/media/stats");
  },

  mediaUsage(id: string): Promise<{ posts: { id: string; title: string; status: string }[]; site_logo: boolean; site_favicon: boolean }> {
    return request(`/api/v1/media/${id}/usage`);
  },

  batchDeleteMedia(ids: string[]): Promise<{ deleted: string[]; failed: { id: string; message: string }[] }> {
    return request("/api/v1/media/batch-delete", { method: "POST", body: JSON.stringify({ ids }) });
  },

  /**
   * Uploads and says whether the library already had these bytes: the
   * server answers 200 with its copy instead of 201 with a new one.
   */
  async uploadMediaDedup(file: File): Promise<{ media: MediaResponse; duplicate: boolean }> {
    const fd = new FormData();
    fd.append("file", file);
    const response = await fetch(`${BASE}/api/v1/media`, { method: "POST", body: fd, credentials: "same-origin" });
    if (!response.ok) {
      let body: Partial<ApiErrorBody> | null = null;
      try {
        body = (await response.json()) as Partial<ApiErrorBody>;
      } catch {
        // non-JSON error body
      }
      throw new ApiError(response.status, body);
    }
    return { media: parseJsonPreservingIds<MediaResponse>(await response.text()), duplicate: response.status === 200 };
  },

  /** Rotate, flip and crop an image in place; derivatives regenerate. */
  editMedia(id: string, body: { rotate?: 0 | 90 | 180 | 270; flip_h?: boolean; flip_v?: boolean; crop?: { x: number; y: number; w: number; h: number } | null }): Promise<MediaResponse> {
    return request(`/api/v1/media/${id}/edit`, { method: "POST", body: JSON.stringify(body) });
  },

  replaceMedia(id: string, file: File): Promise<MediaResponse> {
    const fd = new FormData();
    fd.append("file", file);
    return request(`/api/v1/media/${id}/replace`, { method: "POST", body: fd });
  },

  uploadMedia(file: File): Promise<MediaResponse> {
    const fd = new FormData();
    fd.append("file", file);
    return request("/api/v1/media", { method: "POST", body: fd });
  },

  deleteMedia(id: string): Promise<void> {
    return request(`/api/v1/media/${id}`, { method: "DELETE" });
  },

  listUsers(params: { q?: string; page?: number; per_page?: number } = {}): Promise<{ items: UserResponse[]; total: number }> {
    const qs = new URLSearchParams();
    if (params.q) qs.set("q", params.q);
    if (params.page) qs.set("page", String(params.page));
    qs.set("per_page", String(params.per_page ?? 50));
    return requestWithTotal<UserResponse[]>(`/api/v1/users?${qs.toString()}`).then(({ body, total }) => ({ items: body, total }));
  },

  updateUser(id: string, body: { email?: string; username?: string; display_name?: string; bio?: string }): Promise<UserResponse> {
    return request(`/api/v1/users/${id}`, { method: "PATCH", body: JSON.stringify(body) });
  },

  suspendUser(id: string, suspended: boolean): Promise<void> {
    return request(`/api/v1/users/${id}/suspend`, { method: "POST", body: JSON.stringify({ suspended }) });
  },

  revokeUserSessions(id: string): Promise<{ ended: number }> {
    return request(`/api/v1/users/${id}/sessions/revoke`, { method: "POST" });
  },

  sendResetLink(id: string): Promise<{ sent: "invitation" | "reset" }> {
    return request(`/api/v1/users/${id}/reset-link`, { method: "POST" });
  },

  forgotPassword(email: string): Promise<void> {
    return request("/api/v1/auth/forgot", { method: "POST", body: JSON.stringify({ email }) });
  },

  resetPassword(token: string, password: string): Promise<void> {
    return request("/api/v1/auth/reset", { method: "POST", body: JSON.stringify({ token, password }) });
  },

  /** Whether the site takes new accounts right now, and the password rule to show. */
  registrationInfo(): Promise<RegistrationInfo> {
    return request("/api/v1/auth/registration");
  },

  /**
   * Always resolves (202) when the request is well-formed: the answer is
   * identical whether or not the address already has an account, so it
   * reveals nothing. `website` is a honeypot left for a bot to fill.
   */
  register(body: { email: string; display_name?: string | null; password: string; website?: string }): Promise<{ message: string }> {
    return request("/api/v1/auth/register", { method: "POST", body: JSON.stringify(body) });
  },

  /** Also always 202; same no-enumeration rule as `register`. */
  resendConfirmation(email: string): Promise<{ message: string }> {
    return request("/api/v1/auth/register/resend", { method: "POST", body: JSON.stringify({ email }) });
  },

  /**
   * Confirms an address. With the password chosen at sign-up, it is kept
   * (a wrong one is `400 password_mismatch` and the link still works).
   * Without one, the address is confirmed, any stored password is removed
   * and a set-password link is mailed. Both successes are the same 204.
   */
  verifyEmail(token: string, password?: string): Promise<void> {
    const body = password === undefined ? { token } : { token, password };
    return request("/api/v1/auth/verify", { method: "POST", body: JSON.stringify(body) });
  },

  /**
   * Administrator action: confirms an account directly, removes the password
   * it was registered with and mails the person a link to set one.
   * `link_sent: false` means the account is confirmed but that mail could
   * not be queued. Reach-guarded like the other user actions.
   */
  confirmUser(id: string): Promise<ConfirmResponse> {
    return request(`/api/v1/users/${id}/confirm`, { method: "POST" });
  },

  /** Administrator action: sends a fresh confirmation link. Reach-guarded. */
  resendUserConfirmation(id: string): Promise<void> {
    return request(`/api/v1/users/${id}/resend-confirmation`, { method: "POST" });
  },

  /** `current_password` is required by the server whenever `password` is set. */
  updateMe(body: { display_name?: string; bio?: string; password?: string; current_password?: string; avatar_media_id?: string | null }): Promise<void> {
    return request("/api/v1/users/me", { method: "PUT", body: JSON.stringify(body) });
  },

  listApiKeys(): Promise<{ id: string; name: string; capabilities: string[]; last_used_at: string | null; created_at: string }[]> {
    return request("/api/v1/api-keys");
  },

  createApiKey(body: { name: string; capabilities: string[] }): Promise<{ id: string; name: string; key: string }> {
    return request("/api/v1/api-keys", { method: "POST", body: JSON.stringify(body) });
  },

  revokeApiKey(id: string): Promise<void> {
    return request(`/api/v1/api-keys/${id}`, { method: "DELETE" });
  },


  createUser(body: { email: string; username?: string | null; password?: string | null; role?: string; display_name?: string | null }): Promise<UserResponse> {
    return request("/api/v1/users", { method: "POST", body: JSON.stringify(body) });
  },

  /**
   * When the user owns posts or media, the server answers 409 `conflict`
   * instead of deleting; pass `reassignTo` (another user's id) to hand that
   * content over and retry.
   */
  deleteUser(id: string, reassignTo?: string): Promise<void> {
    const qs = reassignTo === undefined ? "" : `?reassign_to=${encodeURIComponent(reassignTo)}`;
    return request(`/api/v1/users/${id}${qs}`, { method: "DELETE" });
  },

  setUserRole(id: string, role: string): Promise<void> {
    return request(`/api/v1/users/${id}/role`, { method: "PUT", body: JSON.stringify({ role }) });
  },

  /** The five built-in roles, then custom roles by name. */
  listRoles(): Promise<RoleResponse[]> {
    return request("/api/v1/roles");
  },

  createRole(body: { slug: string; name: string; description?: string; capabilities: string[] }): Promise<RoleResponse> {
    return request("/api/v1/roles", { method: "POST", body: JSON.stringify(body) });
  },

  /** Any field left out is unchanged; `capabilities` replaces the whole list when given. */
  updateRole(slug: string, body: { slug?: string; name?: string; description?: string; capabilities?: string[] }): Promise<RoleResponse> {
    return request(`/api/v1/roles/${encodeURIComponent(slug)}`, { method: "PATCH", body: JSON.stringify(body) });
  },

  /** 409 (with the number of users) when the role is still assigned to someone. */
  deleteRole(slug: string): Promise<void> {
    return request(`/api/v1/roles/${encodeURIComponent(slug)}`, { method: "DELETE" });
  },

  getOptions(): Promise<Record<string, unknown>> {
    return request("/api/v1/options");
  },

  /** Writes several options atomically; nothing lands if any value is rejected. */
  putOptions(values: Record<string, unknown>): Promise<void> {
    return request("/api/v1/options", {
      method: "PUT",
      body: JSON.stringify(values),
    });
  },

  putOption(key: string, value: unknown): Promise<void> {
    return request(`/api/v1/options/${key}`, { method: "PUT", body: JSON.stringify(value) });
  },

  listMenus(): Promise<{ id: string; slug: string; name: string }[]> {
    return request("/api/v1/menus");
  },

  browseRegistry(params: { kind?: "plugin" | "theme"; q?: string } = {}): Promise<{
    configured: boolean;
    error: string | null;
    entries: RegistryEntry[];
  }> {
    return request(`/api/v1/registry${queryString(params)}`);
  },

  installFromRegistry(body: {
    kind: "plugin" | "theme";
    name: string;
    version?: string;
    accept_capabilities?: string[];
  }): Promise<{
    kind: "plugin" | "theme";
    name: string;
    version: string;
    capabilities: string[];
    needs_enabling: boolean;
  }> {
    return request("/api/v1/registry/install", {
      method: "POST",
      body: JSON.stringify(body),
    });
  },

  updateStatus(): Promise<{
    current: string;
    latest: string | null;
    update_available: boolean;
    releases: {
      version: string;
      summary: string;
      notes_url: string | null;
      requires_attention: boolean;
    }[];
    environment: {
      mode: "standalone" | "systemd" | "docker" | "kubernetes";
      binary_path: string | null;
      binary_replaceable: boolean;
      restart_supervised: boolean;
    };
    instructions: string[];
    preflight: {
      can_proceed: boolean;
      findings: { name: string; status: "ok" | "warn" | "fail"; detail: string }[];
    };
    channel_error: string | null;
  }> {
    return request("/api/v1/updates");
  },

  applyUpdate(body: { version?: string; skip_backup?: boolean }): Promise<UpdateProgress> {
    return request("/api/v1/updates/apply", {
      method: "POST",
      body: JSON.stringify(body),
    });
  },

  updateProgress(): Promise<UpdateProgress | null> {
    return request("/api/v1/updates/status");
  },

  getLanguage(id: string): Promise<{
    lang: string;
    group: { id: string; lang: string; title: string; slug: string }[];
  }> {
    return request(`/api/v1/posts/${id}/language`);
  },

  putLanguage(
    id: string,
    body: { lang: string; link_slug?: string },
  ): Promise<{
    lang: string;
    group: { id: string; lang: string; title: string; slug: string }[];
  }> {
    return request(`/api/v1/posts/${id}/language`, {
      method: "PUT",
      body: JSON.stringify(body),
    });
  },

  analyticsSummary(days = 30): Promise<{
    today: number;
    total: number;
    series: { day: string; views: number }[];
    top_paths: { name: string; views: number }[];
    top_referrers: { name: string; views: number }[];
  }> {
    return request(`/api/v1/analytics/summary?days=${days}`);
  },

  listSubmissions(): Promise<
    {
      id: string;
      form: string;
      name: string;
      email: string;
      message: string;
      created_at: string;
    }[]
  > {
    return request("/api/v1/audience/submissions");
  },

  deleteSubmission(id: string): Promise<void> {
    return request(`/api/v1/audience/submissions/${id}`, { method: "DELETE" });
  },

  listSubscribers(): Promise<
    {
      id: string;
      email: string;
      status: "pending" | "confirmed" | "unsubscribed";
      created_at: string;
      confirmed_at: string | null;
    }[]
  > {
    return request("/api/v1/audience/subscribers");
  },

  deleteSubscriber(id: string): Promise<void> {
    return request(`/api/v1/audience/subscribers/${id}`, { method: "DELETE" });
  },

  createMenu(body: { slug: string; name: string }): Promise<{ id: string; slug: string }> {
    return request("/api/v1/menus", { method: "POST", body: JSON.stringify(body) });
  },

  getMenu(id: string): Promise<
    [
      { id: string; slug: string; name: string },
      {
        id: string;
        parent_id: string | null;
        label: string;
        url: string;
        sort_order: number;
      }[],
    ]
  > {
    return request(`/api/v1/menus/${id}`);
  },

  addMenuItem(
    menuId: string,
    body: {
      label: string;
      url: string;
      parent_id?: string | null;
      sort_order?: number;
    },
  ): Promise<{ id: string; parent_id: string | null; label: string; url: string; sort_order: number }> {
    return request(`/api/v1/menus/${menuId}/items`, {
      method: "POST",
      body: JSON.stringify(body),
    });
  },

  updateMenuItem(
    itemId: string,
    body: {
      label?: string;
      url?: string;
      parent_id?: string | null;
      detach?: boolean;
      sort_order?: number;
    },
  ): Promise<unknown> {
    return request(`/api/v1/menus/items/${itemId}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    });
  },

  deleteMenuItem(itemId: string): Promise<void> {
    return request(`/api/v1/menus/items/${itemId}`, { method: "DELETE" });
  },

  webhookDeliveries(id: string): Promise<WebhookDelivery[]> {
    return request(`/api/v1/webhooks/${id}/deliveries`);
  },


  updateMedia(
    id: string,
    body: { alt?: string; caption?: string; file_name?: string; focal_x?: number; focal_y?: number },
  ): Promise<MediaResponse> {
    return request(`/api/v1/media/${id}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    });
  },

  deleteMenu(id: string): Promise<void> {
    return request(`/api/v1/menus/${id}`, { method: "DELETE" });
  },

  // ---- AI integrations ----------------------------------------------------

  /** Document-level assist (title, excerpt, seo, tags): copy-into-form. */
  assist<T>(kind: string, content: unknown, vocabulary: string[] = []): Promise<T> {
    return request(`/api/v1/ai/assist/${kind}`, {
      method: "POST",
      body: JSON.stringify({ content, vocabulary }),
    });
  },

  generateAltText(mediaId: string): Promise<{ alt: string | null }> {
    return request(`/api/v1/media/${mediaId}/alt-text`, { method: "POST" });
  },

  transcribeMedia(mediaId: string): Promise<{ queued: boolean }> {
    return request(`/api/v1/media/${mediaId}/transcribe`, { method: "POST" });
  },

  mediaTranscript(mediaId: string): Promise<{ transcript: string | null }> {
    return request(`/api/v1/media/${mediaId}/transcript`);
  },

  readAloud(postId: string): Promise<{ queued: boolean }> {
    return request(`/api/v1/posts/${postId}/read-aloud`, { method: "POST" });
  },

  postAudio(postId: string): Promise<{ media_id: string | null; url?: string; model?: string }> {
    return request(`/api/v1/posts/${postId}/audio`);
  },

  relatedPosts(postId: string, limit = 5): Promise<{ posts: { id: string; title: string; slug: string; url: string }[] }> {
    return request(`/api/v1/posts/${postId}/related${queryString({ limit })}`);
  },

  backfillAi(feature: "embeddings" | "autofill" | "screen", after_id?: string): Promise<{ queued: number; next_after_id: string | null }> {
    return request("/api/v1/ai/backfill", { method: "POST", body: JSON.stringify({ feature, after_id }) });
  },

  embedPost(postId: string): Promise<{ embedded: boolean; reason?: string }> {
    return request(`/api/v1/posts/${postId}/embed`, { method: "POST" });
  },

  autofillPost(postId: string): Promise<{ filled: string[]; reason?: string }> {
    return request(`/api/v1/posts/${postId}/autofill`, { method: "POST" });
  },

  screenComment(commentId: string): Promise<CommentModeration> {
    return request(`/api/v1/comments/${commentId}/screen`, { method: "POST" });
  },

  generateImage(prompt: string, size = "1024x1024"): Promise<MediaResponse> {
    return request("/api/v1/ai/images", {
      method: "POST",
      body: JSON.stringify({ prompt, size }),
    });
  },

  // ---- AI model registry ------------------------------------------------

  /**
   * The public preview page for a token URL the server issued. The API
   * hands back `/api/v1/posts/{id}/preview?token=…`; the page lives at
   * `/preview/{id}?token=…`. Replacing only the prefix left a stray
   * `/preview` segment, so the editor's Preview button opened a 404.
   */
  previewPageUrl(tokenUrl: string): string {
    const m = /\/api\/v1\/posts\/(\d+)\/preview(\?.*)?$/.exec(tokenUrl);
    return m === null ? tokenUrl : `/preview/${m[1]}${m[2] ?? ""}`;
  },

  lockPost(postId: string, force = false): Promise<{ mine: boolean; holder_name: string | null; seen_ago_secs: number | null }> {
    return request(`/api/v1/posts/${postId}/lock${force ? "?force=true" : ""}`, { method: "POST" });
  },
  unlockPost(postId: string): Promise<void> {
    return fetch(`${BASE}/api/v1/posts/${postId}/lock`, { method: "DELETE", credentials: "same-origin", keepalive: true }).then(() => undefined);
  },
  seoSignals(postId: string): Promise<{ inbound_links: number; keyphrase_rivals: { id: string; title: string }[]; redirects_here: string[] }> {
    return request(`/api/v1/posts/${postId}/seo-signals`);
  },
  checkLinks(postId: string): Promise<{ checked_at: string; checked: number; broken: { url: string; status: number }[] }> {
    return request(`/api/v1/posts/${postId}/check-links`, { method: "POST" });
  },
  lastLinkCheck(postId: string): Promise<{ checked_at: string; checked: number; broken: { url: string; status: number }[] }> {
    return request(`/api/v1/posts/${postId}/link-check`);
  },
  /** Title and thumbnail for an embed URL, from the provider's oEmbed. */
  embedPreview(url: string): Promise<{ provider: string; title: string | null; thumbnail_url: string | null; author_name: string | null }> {
    return request(`/api/v1/embeds/preview?url=${encodeURIComponent(url)}`);
  },

  /* ------------------------------------------------------------ setup */
  setupStatus(): Promise<{
    needs_admin: boolean;
    needs_setup: boolean;
    step: string | null;
    instance: string;
    demo?: { email: string; password: string } | null;
  }> {
    return request("/api/v1/setup/status");
  },
  setupClaim(token: string): Promise<void> {
    return request("/api/v1/setup/claim", { method: "POST", body: JSON.stringify({ token }) });
  },
  setupChecks(): Promise<{ name: string; status: string; detail: string }[]> {
    return request("/api/v1/setup/checks");
  },
  /* --------------------------------------------------------- privacy */
  privacyExport(email: string): Promise<PersonalData> {
    return request(`/api/v1/privacy/export?email=${encodeURIComponent(email)}`);
  },
  privacyErase(email: string): Promise<{ comments_anonymised: number; submissions_deleted: number; subscriptions_deleted: number; account_deleted: boolean; note: string }> {
    return request("/api/v1/privacy/erase", { method: "POST", body: JSON.stringify({ email }) });
  },
  privacyPolicyPage(): Promise<{ id: string; slug: string; created: boolean }> {
    return request("/api/v1/privacy/policy-page", { method: "POST" });
  },
  auditLog(limit = 200): Promise<AuditEntry[]> {
    return request(`/api/v1/audit-log?limit=${limit}`);
  },

  /* ----------------------------------------------------------- forms */
  listForms(): Promise<FormDef[]> {
    return request("/api/v1/forms");
  },
  createForm(body: FormInput): Promise<FormDef> {
    return request("/api/v1/forms", { method: "POST", body: JSON.stringify(body) });
  },
  updateForm(id: string, body: FormInput): Promise<FormDef> {
    return request(`/api/v1/forms/${id}`, { method: "PUT", body: JSON.stringify(body) });
  },
  deleteForm(id: string): Promise<void> {
    return request(`/api/v1/forms/${id}`, { method: "DELETE" });
  },
  formSubmissions(id: string): Promise<FormSubmission[]> {
    return request(`/api/v1/forms/${id}/submissions`);
  },
  markSubmissionsRead(ids: string[]): Promise<{ read: number }> {
    return request("/api/v1/forms/submissions/read", { method: "POST", body: JSON.stringify({ ids: ids.map(Number) }) });
  },

  /* -------------------------------------------------------- patterns */
  listPatterns(): Promise<Pattern[]> {
    return request("/api/v1/patterns");
  },
  createPattern(body: PatternInput): Promise<Pattern> {
    return request("/api/v1/patterns", { method: "POST", body: JSON.stringify(body) });
  },
  updatePattern(id: string, body: PatternInput): Promise<Pattern> {
    return request(`/api/v1/patterns/${id}`, { method: "PUT", body: JSON.stringify(body) });
  },
  deletePattern(id: string): Promise<void> {
    return request(`/api/v1/patterns/${id}`, { method: "DELETE" });
  },

  /* ---------------------------------------------------------- export */
  /** What an import made, and every warning (dropped field values included). */
  importArchive(archive: unknown, options: boolean): Promise<components["schemas"]["ImportReport"]> {
    return request(`/api/v1/import?options=${options ? "true" : "false"}`, { method: "POST", body: JSON.stringify(archive) });
  },

  /* ------------------------------------------------------------ mail */
  mailSettings(): Promise<MailSettings> {
    return request("/api/v1/mail/settings");
  },
  mailSettingsSave(input: { host: string; port: number; username: string; from: string; password?: string }): Promise<MailSettings> {
    return request("/api/v1/mail/settings", { method: "PUT", body: JSON.stringify(input) });
  },
  mailTest(to: string): Promise<void> {
    return request("/api/v1/mail/test", { method: "POST", body: JSON.stringify({ to }) });
  },

  /* --------------------------------------------------------- storage */
  storageSettings(): Promise<StorageSettings> {
    return request("/api/v1/media/storage");
  },
  storageSettingsSave(input: StorageInput): Promise<StorageSettings> {
    return request("/api/v1/media/storage", { method: "PUT", body: JSON.stringify(input) });
  },
  storageTest(input: StorageInput): Promise<void> {
    return request("/api/v1/media/storage/test", { method: "POST", body: JSON.stringify(input) });
  },
  storageMigrate(): Promise<MigrationProgress> {
    return request("/api/v1/media/storage/migrate", { method: "POST" });
  },

  registrySources(): Promise<{ marketplace: SourceInfo; updates: SourceInfo }> {
    return request("/api/v1/registry/sources");
  },
  setupVerifyUrl(url: string): Promise<{ reachable: boolean; local: boolean }> {
    return request(`/api/v1/setup/verify-url?url=${encodeURIComponent(url)}`);
  },
  setupStep<T = void>(step: string, body: unknown): Promise<T> {
    return request(`/api/v1/setup/${step}`, { method: "POST", body: JSON.stringify(body ?? {}) });
  },

  /** What AI the editor may offer; readable by any admin-area user. */
  aiAvailable(): Promise<{ text: boolean; vision: boolean; image: boolean; transcription: boolean; speech: boolean; embeddings?: boolean; autofill?: boolean; screening?: boolean }> {
    return request("/api/v1/ai/available");
  },

  aiModels(): Promise<AiRegistry> {
    return request("/api/v1/ai/models");
  },

  saveAiProvider(
    provider: string,
    body: { api_key?: string; base_url?: string; enabled?: boolean },
  ): Promise<AiProvider> {
    return request(`/api/v1/ai/providers/${provider}`, {
      method: "PUT",
      body: JSON.stringify(body),
    });
  },

  deleteAiProvider(provider: string): Promise<void> {
    return request(`/api/v1/ai/providers/${provider}`, { method: "DELETE" });
  },

  testAiProvider(provider: string): Promise<{ ok: boolean; detail: string }> {
    return request(`/api/v1/ai/providers/${provider}/test`, { method: "POST" });
  },

  aiCatalog(provider: string, q: string, kind?: string): Promise<AiCatalogEntry[]> {
    return request(`/api/v1/ai/providers/${provider}/catalog${queryString({ q, kind: kind ?? null })}`);
  },

  registerAiModel(body: {
    provider: string;
    kind: string;
    model: string;
    label?: string;
    settings?: Record<string, unknown>;
    input_cost_per_mtok?: number | null;
    output_cost_per_mtok?: number | null;
    make_default?: boolean;
  }): Promise<AiModel> {
    return request("/api/v1/ai/models", { method: "POST", body: JSON.stringify(body) });
  },

  editAiModel(
    id: string,
    body: {
      label?: string;
      enabled?: boolean;
      settings?: Record<string, unknown>;
      input_cost_per_mtok?: number | null;
      output_cost_per_mtok?: number | null;
    },
  ): Promise<AiModel> {
    return request(`/api/v1/ai/models/${id}`, { method: "PUT", body: JSON.stringify(body) });
  },

  orderAiModels(kind: string, ids: string[]): Promise<void> {
    return request("/api/v1/ai/models/order", { method: "PUT", body: JSON.stringify({ kind, ids }) });
  },

  defaultAiModel(id: string): Promise<AiModel> {
    return request(`/api/v1/ai/models/${id}/default`, { method: "POST" });
  },

  testAiModel(id: string): Promise<{ ok: boolean; detail: string }> {
    return request(`/api/v1/ai/models/${id}/test`, { method: "POST" });
  },

  deleteAiModel(id: string): Promise<void> {
    return request(`/api/v1/ai/models/${id}`, { method: "DELETE" });
  },

  listWebhooks(): Promise<WebhookSummary[]> {
    return request("/api/v1/webhooks");
  },

  createWebhook(body: { url: string; events: string[] }): Promise<WebhookCreated> {
    return request("/api/v1/webhooks", { method: "POST", body: JSON.stringify(body) });
  },

  updateWebhook(id: string, body: { url?: string; events?: string[]; enabled?: boolean }): Promise<WebhookSummary> {
    return request(`/api/v1/webhooks/${id}`, { method: "PATCH", body: JSON.stringify(body) });
  },

  rotateWebhookSecret(id: string): Promise<{ id: string; secret: string; signature_header: string }> {
    return request(`/api/v1/webhooks/${id}/rotate-secret`, { method: "POST" });
  },

  deleteWebhook(id: string): Promise<void> {
    return request(`/api/v1/webhooks/${id}`, { method: "DELETE" });
  },

  testWebhook(id: string): Promise<{ queued: boolean }> {
    return request(`/api/v1/webhooks/${id}/test`, { method: "POST" });
  },

  redeliverWebhook(id: string, deliveryId: string): Promise<{ queued: boolean }> {
    return request(`/api/v1/webhooks/${id}/deliveries/${deliveryId}/redeliver`, { method: "POST" });
  },


  // --- Site health (phase 47) ---------------------------------------------

  siteHealth(): Promise<SiteHealthReport> {
    return request("/api/v1/site-health");
  },
  /** Runs one of the remedies a check offers. */
  siteHealthOp(op: string): Promise<Record<string, number>> {
    if (op === "reindex") return request("/api/v1/search/reindex", { method: "POST" });
    if (op === "cleanup") return request("/api/v1/site-health/cleanup", { method: "POST" });
    return Promise.reject(new Error(`unknown operation ${op}`));
  },

  version(): Promise<{
    version: string;
    migration_version: number | null;
    /** The admin bundle the server currently serves. */
    ui_bundle: string | null;
  }> {
    return request("/api/v1/version");
  },

  // --- Plugins (phase 35) -------------------------------------------------

  listPlugins(): Promise<PluginSummary[]> {
    return request("/api/v1/plugins");
  },

  installPlugin(file: File): Promise<PluginInstallResponse> {
    const form = new FormData();
    form.append("file", file);
    return request("/api/v1/plugins", { method: "POST", body: form });
  },

  inspectPlugin(file: File): Promise<PluginInspection> {
    const form = new FormData();
    form.append("file", file);
    return request("/api/v1/plugins/inspect", { method: "POST", body: form });
  },

  pluginAudit(id: string): Promise<PluginAuditRow[]> {
    return request(`/api/v1/plugins/${id}/audit`);
  },

  setPluginEnabled(id: string, enabled: boolean): Promise<void> {
    return request(`/api/v1/plugins/${id}/${enabled ? "enable" : "disable"}`, {
      method: "POST",
    });
  },

  rollbackPlugin(id: string, version?: string): Promise<void> {
    return request(`/api/v1/plugins/${id}/rollback`, { method: "POST", body: JSON.stringify(version ? { version } : {}) });
  },

  deletePlugin(id: string): Promise<void> {
    return request(`/api/v1/plugins/${id}`, { method: "DELETE" });
  },

  getPluginSetting(id: string, key: string): Promise<unknown> {
    return request(`/api/v1/plugins/settings/${id}/${key}`);
  },

  putPluginSetting(id: string, key: string, value: unknown): Promise<void> {
    return request(`/api/v1/plugins/settings/${id}/${key}`, {
      method: "PUT",
      body: JSON.stringify(value),
    });
  },

  getPluginSettings(id: string): Promise<Record<string, unknown>> {
    return request(`/api/v1/plugins/settings/${id}`);
  },

  getPluginSurface(): Promise<PluginSurface> {
    return request("/api/v1/plugins/surface");
  },

  // --- Content types and fields (phase 99) ----------------------------------

  /** Built-in types first, then plugins', then administrators'. */
  listContentTypes(): Promise<ContentTypeInfo[]> {
    return request("/api/v1/content-types");
  },

  /** 409 when the slug is taken (by a type, a plugin, a taxonomy or stored entries). */
  createContentType(body: {
    slug: string;
    singular: string;
    plural: string;
    description?: string;
    public?: boolean;
    has_archive?: boolean;
  }): Promise<AdminContentType> {
    return request("/api/v1/content-types", { method: "POST", body: JSON.stringify(body) });
  },

  /** Labels and switches only: the slug never changes. */
  updateContentType(
    slug: string,
    body: { singular?: string; plural?: string; description?: string; public?: boolean; has_archive?: boolean },
  ): Promise<AdminContentType> {
    return request(`/api/v1/content-types/${encodeURIComponent(slug)}`, { method: "PUT", body: JSON.stringify(body) });
  },

  /** 409 (naming how many) while any entry of the type exists, trash included. */
  deleteContentType(slug: string): Promise<void> {
    return request(`/api/v1/content-types/${encodeURIComponent(slug)}`, { method: "DELETE" });
  },

  /** A type's fields, in order. */
  listFields(slug: string): Promise<ContentField[]> {
    return request(`/api/v1/content-types/${encodeURIComponent(slug)}/fields`);
  },

  createField(slug: string, body: ContentFieldInput): Promise<ContentField> {
    return request(`/api/v1/content-types/${encodeURIComponent(slug)}/fields`, { method: "POST", body: JSON.stringify(body) });
  },

  /** `options`, when sent, replaces the old ones. 409 for a kind change while entries hold a value. */
  updateField(
    slug: string,
    key: string,
    body: ContentFieldChanges,
  ): Promise<ContentField> {
    return request(`/api/v1/content-types/${encodeURIComponent(slug)}/fields/${encodeURIComponent(key)}`, {
      method: "PUT",
      body: JSON.stringify(body),
    });
  },

  /** Stored values stay (unserved) until cleaned up. */
  deleteField(slug: string, key: string): Promise<void> {
    return request(`/api/v1/content-types/${encodeURIComponent(slug)}/fields/${encodeURIComponent(key)}`, { method: "DELETE" });
  },

  /** Every key of the type, once, in the new order. */
  reorderFields(slug: string, keys: string[]): Promise<ContentField[]> {
    const body: components["schemas"]["ContentFieldOrder"] = { keys };
    return request(`/api/v1/content-types/${encodeURIComponent(slug)}/field-order`, { method: "PUT", body: JSON.stringify(body) });
  },

  /** Keys entries still store values for that no field defines. */
  listOrphans(slug: string): Promise<OrphanValues[]> {
    return request(`/api/v1/content-types/${encodeURIComponent(slug)}/orphans`);
  },

  /** Removes the stored values for a key no field defines. */
  cleanUpOrphan(slug: string, key: string): Promise<components["schemas"]["ContentFieldCleanUp"]> {
    return request(`/api/v1/content-types/${encodeURIComponent(slug)}/orphans/${encodeURIComponent(key)}/clean-up`, { method: "POST" });
  },
};
