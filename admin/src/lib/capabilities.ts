import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/client";

/** Use effective server grants, including per-user overrides. */
export function useCapabilities() {
  const query = useQuery({ queryKey: ["my-caps"], queryFn: () => api.myCaps(), staleTime: 60_000, retry: false });
  return { ...query, can: (capability: string) => query.data?.includes(capability) === true };
}

/**
 * Every capability listed must be held (`string[]`), or at least one of an
 * `anyOf` set — matching an `Access::Cap` vs. `Access::AnyOf` route
 * declaration in `docs/ROUTE-ACCESS.md`.
 */
type Requirement = string[] | { anyOf: string[] };

/**
 * Kept in step with `docs/ROUTE-ACCESS.md`: each page is visible iff the
 * route(s) it reads from would succeed for the caller. A page whose actions
 * need more than its read route stays visible and hides/disables just that
 * action (see the Audience and Media pages) rather than being hidden here.
 */
const requirements: Record<string, Requirement> = {
  posts: ["edit_posts"], pages: ["edit_posts"],
  // Entries of a plugin's or an administrator's content type: the post routes.
  entries: ["edit_posts"],
  // GET /content-types takes edit_posts, but every change on the page needs manage_options.
  "content-types": ["manage_options"],
  // GET /media accepts upload_media or edit_posts; uploading/editing still needs upload_media.
  media: { anyOf: ["upload_media", "edit_posts"] },
  comments: ["moderate_comments"], forms: ["edit_others"],
  // GET /analytics/summary, /audience/submissions and /audience/subscribers all take edit_others;
  // deleting a submission or subscriber needs manage_options.
  audience: ["edit_others"],
  taxonomy: ["manage_categories"], appearance: ["manage_themes"], menus: ["manage_themes"],
  patterns: ["edit_posts"], plugins: ["manage_plugins"], models: ["manage_options"],
  users: ["manage_users"],
  // GET /roles (and every other roles route) takes manage_users, same as the users page.
  roles: ["manage_users"],
  // GET /webhooks (and every other webhooks route) takes manage_plugins, not manage_options.
  webhooks: ["manage_plugins"], health: ["manage_options"],
  privacy: ["manage_users", "manage_options"], settings: ["manage_options"],
};

/** The first path segment under the admin (`/admin/posts/3` → `posts`). */
function segmentOf(path: string): string {
  return path.replace(/^\/admin\/?/, "").replace(/^\//, "").split("/")[0] ?? "";
}

/**
 * Without `view_admin` an account can sign in and manage its own profile,
 * and nothing else: the shell shows the profile page and sign-out, with
 * no navigation, whatever other capabilities the account holds.
 */
export function profileOnly(grants: readonly string[]): boolean {
  return !grants.includes("view_admin");
}

export function canVisit(path: string, grants: readonly string[]): boolean {
  const segment = segmentOf(path);
  const req = requirements[segment] ?? [];
  return Array.isArray(req)
    ? req.every(cap => grants.includes(cap))
    : req.anyOf.some(cap => grants.includes(cap));
}

/** Plain-language copy for one capability, shown on the Roles page. */
export interface CapabilityInfo {
  label: string;
  description: string;
  /**
   * Further areas this capability opens beyond `description`'s headline
   * one, named in plain words — shown as an "Also:" list so an admin
   * granting the capability sees everything it unlocks, not just the
   * obvious part.
   */
  also?: string[];
  /**
   * The top-level `/api/v1` route segments (e.g. `/audience/…` → `audience`)
   * this capability's routes fall under, per `docs/ROUTE-ACCESS.md`. Kept
   * in sync by `src/test/capabilities_route_access.test.ts`, which fails
   * when a route in the doc requires this capability under an area not
   * listed here — a nudge to review `description`/`also` before the drift
   * goes unnoticed.
   */
  areas: string[];
  /**
   * Set on the capabilities that are administrator-level powers: what
   * someone holding this one could do to the whole site. The role editor
   * shows it as a warning ("Administrator-level: …") when the capability
   * is ticked. Mirrors the Roles section of `docs/SECURITY.md`.
   */
  administratorLevel?: string;
  /**
   * What this capability's pages show but only a full administrator (an
   * account holding every capability) may do: the server refuses these
   * with 403 for anyone else (`policy::full_administrator`).
   */
  fullAdministratorOnly?: string[];
}

/**
 * The twelve capabilities the server knows (`Capability::ALL` in
 * `crates/core/src/user/rbac.rs`), each with a short label and a
 * jargon-free description for the role editor's checklist. Order here is
 * the order the checklist renders in, matching the server's own order
 * (and the order a built-in role's `capabilities` array comes back in).
 *
 * Descriptions and `also` lists are drawn from `docs/ROUTE-ACCESS.md`: an
 * admin grants a capability based on this text, so it names every area the
 * capability opens, not just the headline one. `publish_posts` and
 * `delete_posts` gate no route directly (ownership decides them inside
 * the `/posts` handlers already covered by `edit_posts`), so they have no
 * `areas` and no `also`.
 */
export const CAPABILITIES: Record<string, CapabilityInfo> = {
  edit_posts: {
    label: "Write drafts",
    description: "Write and edit their own posts and pages.",
    also: [
      "AI writing assist, SEO tools and link checks",
      "locking a post while editing it, and its revision history",
      "patterns (reusable content blocks) and the list of content types",
      "browsing the media library to insert something (not uploading)",
    ],
    areas: ["ai", "content-types", "embeds", "media", "patterns", "posts"],
  },
  publish_posts: {
    label: "Publish their own work",
    description: "Make their own posts and pages live, not just draft them.",
    areas: [],
  },
  edit_others: {
    label: "Edit anyone's work",
    description: "Edit and publish posts and pages written by other people.",
    also: [
      "forms and their submissions",
      "the audience lists — subscribers' and form submitters' details",
      "analytics",
      "emptying the media trash",
    ],
    areas: ["analytics", "audience", "forms", "media"],
  },
  delete_posts: {
    label: "Delete content",
    description: "Remove posts and pages for good.",
    areas: [],
  },
  manage_categories: {
    label: "Organize categories & tags",
    description: "Create, rename and remove categories and tags.",
    areas: ["terms"],
  },
  moderate_comments: {
    label: "Moderate comments",
    description: "Approve, mark as spam, or remove comments.",
    areas: ["comments"],
  },
  upload_media: {
    label: "Upload media",
    description: "Add images, files and other media to the library.",
    also: [
      "generating images with AI, and other AI media actions (transcribing, alt text, replacing a file)",
    ],
    areas: ["ai", "media"],
  },
  manage_users: {
    label: "Manage people",
    description: "Add, edit and remove accounts, and change their roles.",
    also: [
      "custom roles — creating, editing and deleting them",
      "a person's personal data — exporting or erasing it",
      "password resets, second-factor resets and signing someone out everywhere",
    ],
    areas: ["privacy", "roles", "users"],
  },
  manage_themes: {
    label: "Manage appearance",
    description: "Administrator-level. Install, switch and customize themes.",
    administratorLevel: "they can replace what every visitor sees, on every page, and install theme packages from the marketplace",
    also: [
      "the theme studio and its AI assistant",
      "menus",
      "the plugin & theme marketplace (browsing and installing)",
    ],
    areas: ["menus", "themes", "registry"],
  },
  manage_plugins: {
    label: "Manage plugins",
    description: "Administrator-level. Install, turn on, remove plugins, and manage webhooks.",
    administratorLevel: "they can install code that runs on the server and scripts that run on every page of the site, and send the site's events to any address",
    also: [
      "the plugin & theme marketplace (browsing and installing)",
    ],
    areas: ["plugins", "webhooks", "registry"],
  },
  manage_options: {
    label: "Manage site settings",
    description: "Administrator-level. Change site-wide settings.",
    administratorLevel: "they can download the whole site (every account's email address and all unpublished work), redirect any address on it elsewhere, restyle every page, and replace the AI provider keys",
    also: [
      "exporting a site archive",
      "the audit log",
      "seeing the mail relay's settings (not its password)",
      "AI models, providers, their keys, and AI usage",
      "checking for updates",
      "rebuilding the search index",
      "redirects",
      "site health checks and cleanup",
      "deleting a submission or subscriber outright",
      "the privacy policy page's content",
      "content types and their fields — creating, relabelling and deleting them",
    ],
    fullAdministratorOnly: [
      "changing the mail relay or sending a test message through it",
      "changing the site address",
      "changing where updates and marketplace packages come from, or whose signature is trusted",
      "applying updates",
      "importing a site archive",
    ],
    areas: ["ai", "audience", "audit-log", "content-types", "export", "import", "mail", "options", "privacy", "redirects", "search", "site-health", "updates"],
  },
  view_admin: {
    label: "Use the admin screens",
    description: "Use the admin screens. Without this, people with the role can sign in and manage their own profile, but can't use the admin screens.",
    also: ["whether AI features are available, and a post's related list"],
    areas: ["ai", "posts"],
  },
};

/** The twelve capability names, in the order the checklist renders. */
export const CAPABILITY_ORDER: string[] = Object.keys(CAPABILITIES);

/** A short label for a capability, falling back to its raw name. */
export function capabilityLabel(name: string): string {
  return CAPABILITIES[name]?.label ?? name;
}
