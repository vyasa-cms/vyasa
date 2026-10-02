import * as React from "react";
import { X } from "lucide-react";

import {
  contrastRatio,
  normalizeHex,
  slotValue,
  type ColorPalette,
  type ColorSlot,
  type Diagnostic,
  type FontChoice,
  type ScaleRatio,
  type TokenSet,
} from "@/api/themes";
import { cn } from "@/lib/utils";

const ROLES: { key: keyof ColorPalette; label: string; hint: string }[] = [
  { key: "bg", label: "Background", hint: "Page background" },
  { key: "surface", label: "Surface", hint: "Cards and raised panels" },
  { key: "text", label: "Text", hint: "Body text" },
  { key: "text_muted", label: "Muted text", hint: "Secondary text" },
  { key: "border", label: "Border", hint: "Hairlines and separators" },
  { key: "primary", label: "Primary", hint: "Brand and links" },
  { key: "on_primary", label: "On primary", hint: "Text placed on primary" },
];

const CONTRAST_PAIRS: {
  name: string;
  fg: keyof ColorPalette;
  bg: keyof ColorPalette;
}[] = [
  { name: "Text on background", fg: "text", bg: "bg" },
  { name: "Muted text on background", fg: "text_muted", bg: "bg" },
  { name: "Text on surface", fg: "text", bg: "surface" },
  { name: "Text on primary", fg: "on_primary", bg: "primary" },
];

const FONT_CHOICES: { value: string; label: string }[] = [
  { value: "system_ui", label: "System UI" },
  { value: "serif", label: "Serif" },
  { value: "mono", label: "Monospace" },
  { value: "custom", label: "Custom family…" },
];

const SCALES: { value: string; label: string }[] = [
  { value: "major_second", label: "Major second (1.125)" },
  { value: "minor_third", label: "Minor third (1.2)" },
  { value: "major_third", label: "Major third (1.25)" },
  { value: "perfect_fourth", label: "Perfect fourth (1.333)" },
  { value: "golden", label: "Golden (1.618)" },
  { value: "custom", label: "Custom ratio…" },
];

const fontKind = (f: FontChoice): string =>
  typeof f === "string" ? f : "custom";
const fontCustom = (f: FontChoice): string =>
  typeof f === "string" ? "" : f.custom;
const scaleKind = (s: ScaleRatio): string =>
  typeof s === "string" ? s : "custom";
const scaleCustom = (s: ScaleRatio): number =>
  typeof s === "string" ? 1.25 : s.custom;

const inputClass =
  "h-8 w-full rounded-md border border-input bg-background px-2 text-sm focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring";

/**
 * A hex field that only commits well-formed values, so a half-typed colour
 * never reaches the validator (or the preview) as an error.
 */
function HexInput({
  value,
  onCommit,
  label,
}: {
  value: string;
  onCommit: (hex: string) => void;
  label: string;
}) {
  const [text, setText] = React.useState(value);
  // Follow the prop when it changes from outside (a revert, the assistant)
  // without an effect, so a keystroke is never undone by a stale re-run.
  const [seen, setSeen] = React.useState(value);
  if (seen !== value) {
    setSeen(value);
    setText(value);
  }
  const valid = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(text.trim());
  return (
    <div className="flex min-w-0 items-center gap-1">
      <input
        type="color"
        aria-label={`${label} swatch`}
        value={normalizeHex(value)}
        onChange={(e) => onCommit(e.target.value)}
        className="h-7 w-8 shrink-0 cursor-pointer rounded border bg-transparent p-0"
      />
      <input
        type="text"
        aria-label={label}
        value={text}
        spellCheck={false}
        onChange={(e) => {
          const next = e.target.value;
          setText(next);
          if (/^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(next.trim())) {
            onCommit(next.trim().toLowerCase());
          }
        }}
        className={cn(
          // Flexible, not 5.5rem fixed: in a narrow inspector two fixed
          // fields plus a label overflowed the rail and the dark value was
          // clipped mid-colour.
          "h-7 w-full min-w-0 rounded-md border bg-background px-1.5 font-mono text-[11px]",
          !valid && "border-destructive",
        )}
      />
    </div>
  );
}

function ContrastBadge({ ratio }: { ratio: number | null }) {
  if (ratio === null) return null;
  const ok = ratio >= 4.5;
  return (
    <span
      className={cn(
        "rounded px-1 font-mono text-[10px]",
        ok
          ? "bg-success-subtle text-success"
          : "bg-warning-subtle text-warning",
      )}
      title={ok ? "Passes WCAG AA" : "Below WCAG AA (4.5:1)"}
    >
      {ratio.toFixed(1)}:1
    </span>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="space-y-2">
      <h3 className="text-[10px] font-semibold uppercase tracking-widest text-muted-foreground">
        {title}
      </h3>
      {children}
    </section>
  );
}

function Row({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="grid grid-cols-[7rem_minmax(0,1fr)] items-center gap-2 text-xs">
      <span className="text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

function NumberRow({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (n: number) => void;
}) {
  return (
    <Row label={label}>
      <div className="flex items-center gap-2">
        <input
          type="range"
          aria-label={`${label} slider`}
          min={min}
          max={max}
          step={step}
          value={value}
          onChange={(e) => onChange(Number(e.target.value))}
          className="min-w-0 flex-1"
        />
        <input
          type="number"
          aria-label={label}
          min={min}
          max={max}
          step={step}
          value={value}
          onChange={(e) => {
            const n = Number(e.target.value);
            if (Number.isFinite(n)) onChange(Math.min(max, Math.max(min, n)));
          }}
          className="h-7 w-16 rounded-md border bg-background px-1.5 font-mono text-[11px]"
        />
      </div>
    </Row>
  );
}

export function TokensPanel({
  tokens,
  warnings,
  onChange,
}: {
  tokens: TokenSet;
  warnings: Diagnostic[];
  onChange: (next: TokenSet) => void;
}) {
  const patch = (fn: (t: TokenSet) => TokenSet) =>
    onChange(fn(structuredClone(tokens)));
  const setSlot = (role: keyof ColorPalette, slot: ColorSlot) =>
    patch((t) => {
      t.colors[role] = slot;
      return t;
    });

  return (
    <div className="space-y-5" data-testid="tokens-panel">
      {warnings.length > 0 ? (
        <ul
          className="space-y-1 rounded-md bg-warning-subtle px-2.5 py-2 text-[11px] text-warning"
          role="status"
        >
          {warnings.map((w) => (
            <li key={`${w.path}:${w.message}`}>
              <span className="font-mono">{w.path}</span>: {w.message}
            </li>
          ))}
        </ul>
      ) : null}

      <Section title="Colours">
        {/* Label above its pair, not beside it. A label column plus two
            swatch-and-hex fields does not fit the inspector rail: something
            has to give, and it was the dark value, clipped mid-colour. */}
        <div className="grid grid-cols-2 gap-2 pl-9 text-[10px] uppercase tracking-wider text-muted-foreground">
          <span>Light</span>
          <span>Dark</span>
        </div>
        <ul className="space-y-2.5">
          {ROLES.map(({ key, label, hint }) => {
            const slot = tokens.colors[key];
            return (
              <li key={key} className="space-y-1">
                <span className="block truncate text-xs" title={hint}>
                  {label}
                </span>
                <div className="grid grid-cols-2 items-center gap-2">
                  <HexInput
                    label={`${label} light`}
                    value={slot.light}
                    onCommit={(hex) => setSlot(key, { ...slot, light: hex })}
                  />
                  <div className="flex min-w-0 items-center gap-1">
                    <HexInput
                      label={`${label} dark`}
                      value={slot.dark ?? slot.light}
                      onCommit={(hex) => setSlot(key, { ...slot, dark: hex })}
                    />
                    {slot.dark !== undefined && slot.dark !== null ? (
                      <button
                        type="button"
                        aria-label={`Use the light ${label.toLowerCase()} colour in dark mode too`}
                        title="Same as light"
                        onClick={() => setSlot(key, { light: slot.light })}
                        className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
                      >
                        <X className="h-3 w-3" aria-hidden="true" />
                      </button>
                    ) : null}
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
        <ul className="space-y-1 pt-1 text-[11px]">
          {CONTRAST_PAIRS.map((pair) => (
            <li
              key={pair.name}
              className="flex items-center justify-between gap-2"
            >
              <span className="text-muted-foreground">{pair.name}</span>
              <span className="flex gap-1">
                <ContrastBadge
                  ratio={contrastRatio(
                    slotValue(tokens.colors[pair.fg], false),
                    slotValue(tokens.colors[pair.bg], false),
                  )}
                />
                <ContrastBadge
                  ratio={contrastRatio(
                    slotValue(tokens.colors[pair.fg], true),
                    slotValue(tokens.colors[pair.bg], true),
                  )}
                />
              </span>
            </li>
          ))}
        </ul>
      </Section>

      <Section title="Typography">
        {(["heading", "body"] as const).map((slot) => {
          const choice = tokens.typography[slot];
          return (
            <React.Fragment key={slot}>
              <Row label={slot === "heading" ? "Headings" : "Body"}>
                <select
                  aria-label={`${slot} font`}
                  value={fontKind(choice)}
                  onChange={(e) =>
                    patch((t) => {
                      const v = e.target.value;
                      t.typography[slot] =
                        v === "custom"
                          ? { custom: fontCustom(choice) || "Inter" }
                          : (v as FontChoice);
                      return t;
                    })
                  }
                  className={inputClass}
                >
                  {FONT_CHOICES.map((f) => (
                    <option key={f.value} value={f.value}>
                      {f.label}
                    </option>
                  ))}
                </select>
              </Row>
              {typeof choice !== "string" ? (
                <Row label="Family">
                  <input
                    aria-label={`${slot} font family`}
                    value={choice.custom}
                    onChange={(e) =>
                      patch((t) => {
                        t.typography[slot] = { custom: e.target.value };
                        return t;
                      })
                    }
                    className={inputClass}
                  />
                </Row>
              ) : null}
            </React.Fragment>
          );
        })}
        <NumberRow
          label="Base size"
          value={tokens.typography.base_size_px}
          min={12}
          max={24}
          onChange={(n) =>
            patch((t) => {
              t.typography.base_size_px = n;
              return t;
            })
          }
        />
        <Row label="Scale">
          <select
            aria-label="type scale"
            value={scaleKind(tokens.typography.scale_ratio)}
            onChange={(e) =>
              patch((t) => {
                const v = e.target.value;
                t.typography.scale_ratio =
                  v === "custom"
                    ? { custom: scaleCustom(tokens.typography.scale_ratio) }
                    : (v as ScaleRatio);
                return t;
              })
            }
            className={inputClass}
          >
            {SCALES.map((s) => (
              <option key={s.value} value={s.value}>
                {s.label}
              </option>
            ))}
          </select>
        </Row>
        {typeof tokens.typography.scale_ratio !== "string" ? (
          <NumberRow
            label="Ratio"
            value={tokens.typography.scale_ratio.custom}
            min={1}
            max={2.5}
            step={0.01}
            onChange={(n) =>
              patch((t) => {
                t.typography.scale_ratio = { custom: n };
                return t;
              })
            }
          />
        ) : null}
        <FontFaces tokens={tokens} patch={patch} />
      </Section>

      <Section title="Shape & spacing">
        <NumberRow
          label="Corner radius"
          value={tokens.radius_px}
          min={0}
          max={32}
          onChange={(n) =>
            patch((t) => {
              t.radius_px = n;
              return t;
            })
          }
        />
        <Row label="Shadow">
          <select
            aria-label="shadow"
            value={tokens.shadow}
            onChange={(e) =>
              patch((t) => {
                t.shadow = e.target.value as TokenSet["shadow"];
                return t;
              })
            }
            className={inputClass}
          >
            {(["none", "small", "medium", "large"] as const).map((s) => (
              <option key={s} value={s}>
                {s[0]?.toUpperCase()}
                {s.slice(1)}
              </option>
            ))}
          </select>
        </Row>
        <NumberRow
          label="Spacing unit"
          value={tokens.spacing.unit_px}
          min={2}
          max={16}
          onChange={(n) =>
            patch((t) => {
              t.spacing.unit_px = n;
              return t;
            })
          }
        />
      </Section>

      <Section title="Layout">
        <NumberRow
          label="Content width"
          value={tokens.layout.content_width_px}
          min={480}
          max={1920}
          step={10}
          onChange={(n) =>
            patch((t) => {
              t.layout.content_width_px = n;
              return t;
            })
          }
        />
        <NumberRow
          label="Sidebar width"
          value={tokens.layout.sidebar_width_px}
          min={160}
          max={420}
          step={10}
          onChange={(n) =>
            patch((t) => {
              t.layout.sidebar_width_px = n;
              return t;
            })
          }
        />
        <NumberRow
          label="Phone below"
          value={tokens.layout.breakpoint_sm_px}
          min={320}
          max={1920}
          step={10}
          onChange={(n) =>
            patch((t) => {
              t.layout.breakpoint_sm_px = n;
              return t;
            })
          }
        />
        <NumberRow
          label="Tablet below"
          value={tokens.layout.breakpoint_md_px}
          min={320}
          max={1920}
          step={10}
          onChange={(n) =>
            patch((t) => {
              t.layout.breakpoint_md_px = n;
              return t;
            })
          }
        />
        <Row label="Density">
          <select
            aria-label="density"
            value={tokens.layout.density}
            onChange={(e) =>
              patch((t) => {
                t.layout.density = e.target
                  .value as TokenSet["layout"]["density"];
                return t;
              })
            }
            className={inputClass}
          >
            <option value="compact">Compact</option>
            <option value="comfortable">Comfortable</option>
            <option value="spacious">Spacious</option>
          </select>
        </Row>
        <Row label="Direction">
          <select
            aria-label="text direction"
            value={tokens.direction}
            onChange={(e) =>
              patch((t) => {
                t.direction = e.target.value as TokenSet["direction"];
                return t;
              })
            }
            className={inputClass}
          >
            <option value="ltr">Left to right</option>
            <option value="rtl">Right to left</option>
          </select>
        </Row>
      </Section>
    </div>
  );
}

function FontFaces({
  tokens,
  patch,
}: {
  tokens: TokenSet;
  patch: (fn: (t: TokenSet) => TokenSet) => void;
}) {
  const faces = tokens.typography.font_faces;
  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <span className="text-xs text-muted-foreground">Web fonts</span>
        <button
          type="button"
          onClick={() =>
            patch((t) => {
              t.typography.font_faces.push({ family: "", src: "https://" });
              return t;
            })
          }
          className="text-[11px] text-primary hover:underline"
        >
          Add @font-face
        </button>
      </div>
      {faces.map((face, i) => (
        <div key={i} className="flex items-center gap-1">
          <input
            aria-label={`font face ${i + 1} family`}
            placeholder="Family"
            value={face.family}
            onChange={(e) =>
              patch((t) => {
                const f = t.typography.font_faces[i];
                if (f !== undefined) f.family = e.target.value;
                return t;
              })
            }
            className={cn(inputClass, "w-28")}
          />
          <input
            aria-label={`font face ${i + 1} source`}
            placeholder="/theme-assets/fonts/font.woff2"
            value={face.src ?? ""}
            onChange={(e) =>
              patch((t) => {
                const f = t.typography.font_faces[i];
                if (f !== undefined) f.src = e.target.value;
                return t;
              })
            }
            className={inputClass}
          />
          <button
            type="button"
            aria-label={`remove font face ${i + 1}`}
            onClick={() =>
              patch((t) => {
                t.typography.font_faces.splice(i, 1);
                return t;
              })
            }
            className="rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <X className="h-3.5 w-3.5" aria-hidden="true" />
          </button>
        </div>
      ))}
      {faces.length > 0 ? (
        <p className="text-[11px] text-muted-foreground">
          Sources may use HTTPS or /theme-assets/fonts/. Pick “Custom family” above to use one.
        </p>
      ) : null}
    </div>
  );
}
