import * as React from "react";

import { blankRow, fieldsFromSchema, withAttr, type Attrs, type JsonValue, type SchemaField } from "@/lib/pluginSchema";

const input =
  "h-8 w-full rounded-md border border-input bg-background px-2 text-sm focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring";
const area =
  "w-full rounded-md border border-input bg-background p-2 text-sm focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring";
const smallButton = "rounded border px-2 py-1 text-xs hover:bg-muted";

function text(v: JsonValue | undefined): string {
  if (typeof v === "string") return v;
  return v === undefined || v === null ? "" : String(v);
}

function Labelled({ id, field, children }: { id: string; field: SchemaField; children: React.ReactNode }) {
  return (
    <div className="space-y-1">
      <label htmlFor={id} className="block text-xs font-medium">
        {field.label}
      </label>
      {children}
      {field.help === undefined ? null : <p className="text-xs text-muted-foreground">{field.help}</p>}
    </div>
  );
}

function JsonArea({ id, field, value, onChange }: { id: string; field: SchemaField; value: JsonValue | undefined; onChange: (v: JsonValue) => void }) {
  const [draft, setDraft] = React.useState(value === undefined ? "" : JSON.stringify(value, null, 2));
  const [error, setError] = React.useState<string | null>(null);
  return (
    <Labelled id={id} field={field}>
      <textarea
        id={id}
        rows={4}
        spellCheck={false}
        className={`${area} font-mono text-xs`}
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => {
          if (draft.trim() === "") {
            setError(null);
            onChange("");
            return;
          }
          try {
            onChange(JSON.parse(draft) as JsonValue);
            setError(null);
          } catch {
            setError("Not valid JSON; the value was not changed.");
          }
        }}
      />
      {error === null ? null : (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}
    </Labelled>
  );
}

function FieldEditor({ field, value, onChange }: { field: SchemaField; value: JsonValue | undefined; onChange: (v: JsonValue) => void }) {
  const id = React.useId();
  switch (field.kind) {
    case "text":
      return (
        <Labelled id={id} field={field}>
          <input id={id} className={input} value={text(value)} onChange={(e) => onChange(e.target.value)} />
        </Labelled>
      );
    case "multiline":
    case "code":
      return (
        <Labelled id={id} field={field}>
          <textarea
            id={id}
            rows={field.kind === "code" ? 6 : 3}
            spellCheck={field.kind !== "code"}
            className={field.kind === "code" ? `${area} font-mono text-xs` : area}
            value={text(value)}
            onChange={(e) => onChange(e.target.value)}
          />
        </Labelled>
      );
    case "number":
      return (
        <Labelled id={id} field={field}>
          <input
            id={id}
            type="number"
            className={input}
            value={text(value)}
            onChange={(e) => onChange(e.target.value === "" ? "" : Number(e.target.value))}
          />
        </Labelled>
      );
    case "boolean":
      return (
        <div className="flex items-center gap-2">
          <input id={id} type="checkbox" checked={value === true} onChange={(e) => onChange(e.target.checked)} />
          <label htmlFor={id} className="text-xs font-medium">
            {field.label}
          </label>
        </div>
      );
    case "choice":
      return (
        <Labelled id={id} field={field}>
          <select id={id} className={input} value={text(value)} onChange={(e) => onChange(e.target.value)}>
            <option value="" />
            {(field.options ?? []).map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </select>
        </Labelled>
      );
    case "list": {
      const items = Array.isArray(value) ? value.map((v) => text(v)) : [];
      return (
        <fieldset className="space-y-1">
          <legend className="text-xs font-medium">{field.label}</legend>
          {items.map((item, i) => (
            <div key={i} className="flex gap-1">
              <input
                className={input}
                aria-label={`${field.label} ${i + 1}`}
                value={item}
                onChange={(e) => onChange(items.map((x, j) => (j === i ? e.target.value : x)))}
              />
              <button type="button" className={smallButton} aria-label={`Remove ${field.label} ${i + 1}`} onClick={() => onChange(items.filter((_, j) => j !== i))}>
                Remove
              </button>
            </div>
          ))}
          <button type="button" className={smallButton} aria-label={`Add ${field.label}`} onClick={() => onChange([...items, ""])}>
            Add
          </button>
        </fieldset>
      );
    }
    case "rows": {
      const columns = field.columns ?? [];
      const rows = (Array.isArray(value) ? value : []) as Attrs[];
      const setCell = (r: number, key: string, v: JsonValue) => onChange(rows.map((row, j) => (j === r ? { ...row, [key]: v } : row)));
      return (
        <fieldset className="space-y-1">
          <legend className="text-xs font-medium">{field.label}</legend>
          {rows.map((row, r) => (
            <div key={r} className="grid gap-1 rounded border p-2" style={{ gridTemplateColumns: `repeat(${Math.max(columns.length, 1)}, minmax(0, 1fr)) auto` }}>
              {columns.map((c) =>
                c.kind === "boolean" ? (
                  <label key={c.key} className="flex items-center gap-1 text-xs">
                    <input
                      type="checkbox"
                      aria-label={`${field.label} ${r + 1} ${c.label}`}
                      checked={row[c.key] === true}
                      onChange={(e) => setCell(r, c.key, e.target.checked)}
                    />
                    {c.label}
                  </label>
                ) : (
                  <input
                    key={c.key}
                    className={input}
                    placeholder={c.label}
                    aria-label={`${field.label} ${r + 1} ${c.label}`}
                    value={text(row[c.key])}
                    onChange={(e) => setCell(r, c.key, c.kind === "number" ? Number(e.target.value) : e.target.value)}
                  />
                ),
              )}
              <button type="button" className={smallButton} aria-label={`Remove ${field.label} ${r + 1}`} onClick={() => onChange(rows.filter((_, j) => j !== r))}>
                Remove
              </button>
            </div>
          ))}
          <button type="button" className={smallButton} aria-label={`Add ${field.label}`} onClick={() => onChange([...rows, blankRow(columns)])}>
            Add
          </button>
        </fieldset>
      );
    }
    default:
      return <JsonArea id={id} field={field} value={value} onChange={onChange} />;
  }
}

/**
 * The attribute form of a plugin block, built from the `attrs-schema` the
 * plugin declared. Renders nothing for a plugin that declared none (the
 * panel then shows only the JSON editor it always had).
 */
export function PluginAttrsForm({ schema, attrs, onChange }: { schema: unknown; attrs: Attrs; onChange: (next: Attrs) => void }) {
  const fields = React.useMemo(() => fieldsFromSchema(schema), [schema]);
  if (fields.length === 0) return null;
  return (
    <div className="space-y-3" data-testid="plugin-attrs-form">
      {fields.map((f) => (
        <FieldEditor key={f.key} field={f} value={attrs[f.key]} onChange={(v) => onChange(withAttr(attrs, f.key, v))} />
      ))}
    </div>
  );
}
