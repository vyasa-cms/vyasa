import type { ThemeAssets } from "@/api/themes";

/** Bounds the server enforces, mirrored so the editor can say so first. */
const MAX_CSS = 256 * 1024;
const MAX_JS = 256 * 1024;

/**
 * The theme's own stylesheet and script.
 *
 * Distinct from Appearance → Custom CSS: that belongs to the *site* and
 * survives a theme change, this belongs to the theme and is versioned,
 * previewed and rolled back with it. Design tokens cover colour, type and
 * spacing; this is for the rules tokens cannot express.
 */
export function AssetsPanel({
  assets,
  onChange,
}: {
  assets: ThemeAssets;
  onChange: (next: ThemeAssets) => void;
}) {
  return (
    <div className="space-y-5" data-testid="assets-panel">
      <Editor
        label="Stylesheet"
        hint="Loaded on every page this theme renders. Design tokens already cover colour, type and spacing — use this for what they cannot express."
        language="css"
        value={assets.css}
        max={MAX_CSS}
        testId="theme-css"
        onChange={(css) => onChange({ ...assets, css })}
      />
      <Editor
        label="Script"
        hint="Runs on every page. It is served from your own site and versioned with the theme, so rolling the theme back rolls this back too."
        language="js"
        value={assets.js}
        max={MAX_JS}
        testId="theme-js"
        onChange={(js) => onChange({ ...assets, js })}
      />
    </div>
  );
}

function Editor({
  label,
  hint,
  language,
  value,
  max,
  testId,
  onChange,
}: {
  label: string;
  hint: string;
  language: "css" | "js";
  value: string;
  max: number;
  testId: string;
  onChange: (next: string) => void;
}) {
  const bytes = new TextEncoder().encode(value).length;
  const over = bytes > max;
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-sm font-medium">{label}</span>
      <span className="text-xs text-muted-foreground">{hint}</span>
      <textarea
        aria-label={label}
        data-testid={testId}
        value={value}
        spellCheck={false}
        rows={14}
        onChange={(e) => onChange(e.target.value)}
        className={
          "w-full rounded-md border bg-background p-2 font-mono text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring" +
          (over ? " border-destructive" : "")
        }
        placeholder={
          language === "css" ? ".vy-card { … }" : "// runs on every page"
        }
      />
      <span
        className={
          "text-xs " + (over ? "text-destructive" : "text-muted-foreground")
        }
        role={over ? "alert" : undefined}
      >
        {over
          ? `${bytes.toLocaleString()} bytes — the maximum is ${max.toLocaleString()}, so this will be refused`
          : `${bytes.toLocaleString()} of ${max.toLocaleString()} bytes`}
      </span>
    </label>
  );
}
