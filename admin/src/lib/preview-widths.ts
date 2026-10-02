import type { TokenSet } from "@/api/themes";

/** One preview size, with the width the frame is actually set to. */
export interface PreviewWidth {
  key: "desktop" | "tablet" | "phone";
  label: string;
  /** CSS width for the frame. */
  width: string;
  /** What that width is in pixels, for the label. `null` = fills the pane. */
  px: number | null;
}

/**
 * Preview widths derived from the theme's own breakpoints.
 *
 * Fixed widths would make the labels lie: a theme whose phone breakpoint is
 * 900px would show tablet rules under a "Phone" label, and an author
 * checking `hide_on` would see the wrong thing. Each size is picked to sit
 * unambiguously inside its band.
 */
export function previewWidths(tokens: TokenSet): PreviewWidth[] {
  const sm = tokens.layout.breakpoint_sm_px;
  const md = tokens.layout.breakpoint_md_px;
  // Comfortably below the phone breakpoint, but never narrower than the
  // smallest screen anyone designs for.
  const phone = Math.max(320, Math.min(390, Math.round(sm) - 40));
  // Midway between the two, so it is unambiguously the middle band.
  const tablet = Math.round((sm + md) / 2);
  return [
    { key: "desktop", label: "Desktop", width: "100%", px: null },
    { key: "tablet", label: "Tablet", width: `${tablet}px`, px: tablet },
    { key: "phone", label: "Phone", width: `${phone}px`, px: phone },
  ];
}
