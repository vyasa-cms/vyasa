import { ArrowDown, ArrowUp, Plus, Trash2 } from "lucide-react";

import type { SettingsSchema } from "@/api/themes";
import { Button } from "@/components/ui/button";

type Item = Record<string, unknown>;

const inputClass =
  "h-6 w-full rounded border border-input bg-background px-1.5 text-[11px] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

/**
 * Repeating rows for a section's `items` array.
 *
 * The marketing sections are lists of small records — feature cards, stats,
 * questions — and the schema-driven form only understood scalars, so those
 * sections were configurable by the assistant but not by a person. This is
 * the missing half.
 */
export function ItemsEditor({
  schema,
  items,
  onChange,
  label,
}: {
  schema: SettingsSchema;
  items: Item[];
  onChange: (next: Item[]) => void;
  label: string;
}) {
  // The element shape comes from the schema, so a new section kind gets an
  // editor without any code here changing.
  const itemSchema = (schema.items ?? {}) as SettingsSchema;
  const fields = Object.entries(itemSchema.properties ?? {});
  const required = new Set(itemSchema.required ?? []);
  const max = schema.maxItems ?? 12;

  const setField = (index: number, key: string, value: string) => {
    const next = items.map((item, i) =>
      i === index ? { ...item, [key]: value } : item,
    );
    onChange(next);
  };

  const move = (index: number, direction: -1 | 1) => {
    const to = index + direction;
    if (to < 0 || to >= items.length) return;
    const next = [...items];
    const [item] = next.splice(index, 1);
    if (item === undefined) return;
    next.splice(to, 0, item);
    onChange(next);
  };

  if (fields.length === 0) return null;

  return (
    <div className="space-y-1.5" data-testid="items-editor">
      <div className="flex items-center gap-2">
        <span className="text-[11px] text-muted-foreground">{label}</span>
        <span className="text-[10px] text-muted-foreground">
          {items.length}/{max}
        </span>
      </div>

      <ol className="space-y-1.5">
        {items.map((item, i) => (
          <li key={i} className="rounded border bg-muted/30 p-1.5">
            <div className="mb-1 flex items-center gap-0.5">
              <span className="text-[10px] font-medium text-muted-foreground">
                {i + 1}
              </span>
              <div className="ml-auto flex gap-0.5">
                <button
                  type="button"
                  aria-label={`Move item ${i + 1} up`}
                  disabled={i === 0}
                  onClick={() => move(i, -1)}
                  className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-30"
                >
                  <ArrowUp className="h-3 w-3" aria-hidden="true" />
                </button>
                <button
                  type="button"
                  aria-label={`Move item ${i + 1} down`}
                  disabled={i === items.length - 1}
                  onClick={() => move(i, 1)}
                  className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-30"
                >
                  <ArrowDown className="h-3 w-3" aria-hidden="true" />
                </button>
                <button
                  type="button"
                  aria-label={`Remove item ${i + 1}`}
                  onClick={() => onChange(items.filter((_, j) => j !== i))}
                  className="rounded p-0.5 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                >
                  <Trash2 className="h-3 w-3" aria-hidden="true" />
                </button>
              </div>
            </div>

            <div className="grid gap-1">
              {fields.map(([key, spec]) => (
                <label key={key} className="grid gap-0.5">
                  <span className="text-[10px] text-muted-foreground">
                    {key.replace(/_/g, " ")}
                    {required.has(key) ? " *" : ""}
                  </span>
                  {spec.enum ? (
                    <select aria-label={`Item ${i + 1} ${key}`} value={typeof item[key] === "string" ? item[key] as string : ""} onChange={(e) => setField(i, key, e.target.value)} className={inputClass}>
                      <option value="">Choose…</option>
                      {spec.enum.map((value) => <option key={value} value={value}>{value.replace(/-/g, " ")}</option>)}
                    </select>
                  ) : spec.format === "multiline" ? (
                    <textarea aria-label={`Item ${i + 1} ${key}`} value={typeof item[key] === "string" ? item[key] as string : ""} maxLength={spec.maxLength} rows={5} onChange={(e) => setField(i, key, e.target.value)} className={`${inputClass} h-auto font-mono`} />
                  ) : (
                    <input aria-label={`Item ${i + 1} ${key}`} value={typeof item[key] === "string" ? item[key] as string : ""} maxLength={spec.maxLength} onChange={(e) => setField(i, key, e.target.value)} className={inputClass} />
                  )}
                </label>
              ))}
            </div>
          </li>
        ))}
      </ol>

      <Button
        variant="outline"
        size="sm"
        className="h-6 w-full text-[11px]"
        disabled={items.length >= max}
        onClick={() => {
          // Seed the required fields so the section validates the moment it
          // is added, rather than after the author guesses what is missing.
          const blank: Item = {};
          for (const key of required) blank[key] = "";
          onChange([...items, blank]);
        }}
      >
        <Plus className="h-3 w-3" aria-hidden="true" />
        Add
      </Button>
    </div>
  );
}
