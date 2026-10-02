/** Mirrors the validated server pattern for unsaved editor previews. */
export function postPath(type: string, slug: string, instant: string, pattern = "/post/{slug}"): string {
  if (type === "page") return `/${slug}`;
  if (type !== "post") return `/${type}/${slug}`;
  const date = new Date(instant);
  return pattern.replace("{slug}", slug).replace("{type}", type)
    .replace("{year}", String(date.getUTCFullYear()))
    .replace("{month}", String(date.getUTCMonth() + 1).padStart(2, "0"));
}
