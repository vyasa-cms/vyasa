import type { TemplateType } from "@/api/themes";

/**
 * Which template the preview is showing.
 *
 * Clicking a section on the canvas has to find that section in the layout,
 * and section ids are only unique within one template — every template has
 * a `header`. So the click alone is ambiguous; the path being previewed is
 * what disambiguates it.
 *
 * The paths come from the preview's own page list, so this mirrors that
 * list rather than guessing at arbitrary URLs.
 */
export function templateForPath(path: string): TemplateType {
  const clean = path.split("?")[0] ?? "/";
  if (clean === "/") return "index";
  if (path.startsWith("/search")) return "search";
  if (clean.startsWith("/post/")) return "single";
  if (clean.startsWith("/category/") || clean.startsWith("/tag/")) return "archive";
  if (clean.startsWith("/archive/")) return "archive";
  // The preview offers one deliberately missing page to exercise the 404.
  if (clean === "/studio-preview-missing-page") return "not-found";
  // Two segments that matched nothing above is a custom type's entry
  // (/book/dune) — rendered by the single template, or its per-type
  // override, which shares the single layout tree.
  if (clean.split("/").filter(Boolean).length === 2) return "single";
  return "page";
}
