import { Paintbrush, X } from "lucide-react";

import {
  SCOPE_ROLES,
  type ScopeRole,
  type StyleScope,
  type TokenSet,
} from "@/api/themes";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const ROLE_LABELS: Record<ScopeRole, string> = {
  bg: "Background",
  surface: "Cards",
  text: "Text",
  text_muted: "Secondary text",
  border: "Hairlines",
  primary: "Accent",
  on_primary: "On accent",
};

/**
 * Ready-made scopes for the two things people actually want.
 *
 * Both are written as role references rather than literals, so they keep
 * tracking the palette instead of freezing today's colours into the layout.
 */
const PRESETS: { label: string; hint: string; scope: StyleScope }[] = [
  {
    label: "Dark band",
    hint: "Inverts text and background",
    scope: { bg: "$text", text: "$bg", padding_y: 8 },
  },
  {
    label: "Tinted band",
    hint: "Sits on the card colour",
    scope: { bg: "$surface", padding_y: 6 },
  },
];

/** The colour a role resolves to right now, for the swatch. */
function preview(value: string | undefined, tokens: TokenSet): string | null {
  if (value === undefined || value === "") return null;
  if (!value.startsWith("$")) return /^#[0-9a-f]{3,6}$/i.test(value) ? value : null;
  const role = value.slice(1).replace("-", "_") as keyof TokenSet["colors"];
  const slot = tokens.colors[role];
  return slot?.light ?? null;
}

/**
 * Per-section colour overrides.
 *
 * Values may be a hex literal or `$role`. The reference form is offered first
 * because it is the one that survives a palette change — the literal form is
 * there for the cases where an author really does mean one exact colour.
 */
export function ScopeEditor({
  scope,
  tokens,
  onChange,
}: {
  scope: StyleScope | undefined;
  tokens: TokenSet;
  onChange: (next: StyleScope | undefined) => void;
}) {
  const active = scope ?? {};
  const isSet = scope !== undefined && Object.keys(scope).length > 0;

  const set = (key: keyof StyleScope, value: unknown) => {
    const next: StyleScope = { ...active };
    if (value === "" || value === undefined) delete next[key];
    else Object.assign(next, { [key]: value });
    onChange(Object.keys(next).length === 0 ? undefined : next);
  };

  return (
    <div className="space-y-2" data-testid="scope-editor">
      <div className="flex items-center gap-1.5">
        <Paintbrush className="h-3.5 w-3.5 text-muted-foreground" aria-hidden="true" />
        <span className="text-[11px] font-medium">Style scope</span>
        {isSet ? (
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto h-6 px-1.5 text-[10px] text-muted-foreground"
            onClick={() => onChange(undefined)}
          >
            <X className="h-3 w-3" aria-hidden="true" />
            Clear
          </Button>
        ) : null}
      </div>

      <label className="grid gap-1 text-[11px]">
        Entrance motion
        <select aria-label="Entrance motion" value={active.motion ?? ""} onChange={(e) => set("motion", e.target.value)} className="rounded border bg-background p-1">
          <option value="">None</option><option value="fade">Fade in</option><option value="rise">Rise in</option>
        </select>
        <span className="text-muted-foreground">Respects the visitor’s reduced-motion preference.</span>
      </label>
      {!isSet ? (
        <div className="space-y-1.5">
          <p className="text-[11px] text-muted-foreground">
            Restyle this section and everything inside it.
          </p>
          <div className="flex flex-wrap gap-1">
            {PRESETS.map((p) => (
              <button
                key={p.label}
                type="button"
                title={p.hint}
                onClick={() => onChange(p.scope)}
                className="rounded-md border px-2 py-1 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground"
              >
                {p.label}
              </button>
            ))}
          </div>
        </div>
      ) : (
        <div className="space-y-1.5">
          {SCOPE_ROLES.map((role) => {
            const value = active[role] ?? "";
            const swatch = preview(value, tokens);
            return (
              <label
                key={role}
                className="grid grid-cols-[5.5rem_1fr_1.25rem] items-center gap-1.5 text-[11px]"
              >
                <span className="text-muted-foreground">{ROLE_LABELS[role]}</span>
                <input
                  aria-label={`${ROLE_LABELS[role]} for this section`}
                  value={value}
                  spellCheck={false}
                  placeholder="$text or #101317"
                  onChange={(e) => set(role, e.target.value)}
                  className={cn(
                    "h-6 w-full rounded border border-input bg-background px-1.5 font-mono text-[10px]",
                    "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                  )}
                />
                <span
                  aria-hidden="true"
                  title={swatch ?? "not set"}
                  className="h-4 w-4 rounded border"
                  style={swatch === null ? undefined : { background: swatch }}
                />
              </label>
            );
          })}

          <label className="grid grid-cols-[5.5rem_1fr_1.25rem] items-center gap-1.5 text-[11px]">
            <span className="text-muted-foreground">Padding</span>
            <input
              type="number"
              min={0}
              max={24}
              aria-label="Vertical padding in spacing units"
              value={active.padding_y ?? ""}
              onChange={(e) =>
                set("padding_y", e.target.value === "" ? "" : Number(e.target.value))
              }
              className="h-6 w-full rounded border border-input bg-background px-1.5 text-[10px] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            />
            <span className="text-[9px] text-muted-foreground">u</span>
          </label>

          <p className="text-[10px] leading-snug text-muted-foreground">
            Write <code className="font-mono">$text</code> to follow a theme
            colour, or a hex value to fix one. References keep working when the
            palette changes.
          </p>
        </div>
      )}
    </div>
  );
}
