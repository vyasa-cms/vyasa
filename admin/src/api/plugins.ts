import { api, type PluginInstallResponse, type PluginSummary, type PluginSurface } from "./client";

export type {
  PluginField,
  PluginForm,
  PluginSurface,
} from "./client";

export type { PluginSummary, PluginInspection, PluginAuditRow } from "./client";

/** Plain-language descriptions for capability strings. */
const CAPABILITY_LABELS: Record<string, string> = {
  "db:read:posts": "Read published posts",
  "db:read:options": "Read public site options",
  "db:read:comments": "Read approved comments (no email addresses)",
  "db:write:posts": "Create and edit drafts it made itself",
  "db:publish:posts": "Publish the posts it made",
  "kv:read": "Read its own private storage",
  "kv:write": "Write its own private storage",
  "db:write:meta": "Attach its own notes to posts",
  "viewer:read": "See who is signed in (name and role, never the email)",
  "mail:send": "Email you or a registered user (nobody else)",
  "log:write": "Write log messages",
  "ai:text": "Ask the site's AI text model (budgeted, logged under the plugin)",
  "event:emit": "Emit domain events",
  "html:page": "Rewrite the HTML of every public page",
  "assets:script": "Run its own JavaScript on public pages",
};

export function capabilityLabel(cap: string): string {
  if (CAPABILITY_LABELS[cap] !== undefined) return CAPABILITY_LABELS[cap];
  if (cap.startsWith("net:fetch:")) {
    const pattern = cap.slice("net:fetch:".length);
    return `Fetch from hosts matching ${pattern}`;
  }
  return cap;
}

export const listPlugins = (): Promise<PluginSummary[]> => api.listPlugins();
export const installPlugin = (file: File): Promise<PluginInstallResponse> =>
  api.installPlugin(file);
export const inspectPlugin = (file: File) => api.inspectPlugin(file);
export const pluginAudit = (id: string) => api.pluginAudit(id);
export const setPluginEnabled = (id: string, enabled: boolean): Promise<void> =>
  api.setPluginEnabled(id, enabled);
export const rollbackPlugin = (id: string, version?: string): Promise<void> =>
  api.rollbackPlugin(id, version);
export const deletePlugin = (id: string): Promise<void> => api.deletePlugin(id);
export const putPluginSetting = (
  id: string,
  key: string,
  value: unknown,
): Promise<void> => api.putPluginSetting(id, key, value);
export const getPluginSettings = (
  id: string,
): Promise<Record<string, unknown>> => api.getPluginSettings(id);
export const getPluginSurface = (): Promise<PluginSurface> =>
  api.getPluginSurface();
