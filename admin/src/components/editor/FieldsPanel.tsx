import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, FileText, ListChecks, Search, X } from "lucide-react";
import { api, type ContentField, type FieldValues } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { MediaPicker } from "./MediaPicker";

/**
 * The entry's custom fields, one input per kind.
 *
 * Values are kept typed (numbers as numbers, yes/no as booleans, several
 * choices as a list) so the server receives what it validates; an emptied
 * input removes the value rather than sending "". Required fields are
 * marked but never block saving a draft: the server asks for them only
 * when publishing or scheduling, and its messages land next to the field.
 */
export function FieldsPanel({
  fields,
  values,
  onChange,
  errors,
  missing,
  disabled = false,
}: {
  fields: ContentField[];
  values: FieldValues;
  /** `undefined` removes the value. */
  onChange: (key: string, value: unknown) => void;
  errors: Record<string, string>;
  /** Media and entry fields whose item no longer exists. */
  missing: string[];
  disabled?: boolean;
}) {
  return (
    <section className="rounded-lg border bg-card" data-testid="fields-panel" aria-labelledby="fields-panel-title">
      <div className="flex items-center gap-2 border-b px-4 py-3 text-sm font-medium">
        <ListChecks className="h-4 w-4 text-muted-foreground" aria-hidden="true" />
        <h2 id="fields-panel-title">Fields</h2>
        {fields.some((f) => f.required) ? (
          <span className="ml-auto text-[11px] font-normal text-muted-foreground">
            <span className="text-destructive" aria-hidden="true">*</span> needed to publish; drafts save without them
          </span>
        ) : null}
      </div>
      <div className="grid gap-4 p-4 sm:grid-cols-2">
        {fields.map((f) => (
          <FieldInput
            key={f.key}
            field={f}
            value={values[f.key]}
            onChange={(v) => onChange(f.key, v)}
            error={errors[f.key] ?? null}
            missing={missing.includes(f.key)}
            disabled={disabled}
          />
        ))}
      </div>
    </section>
  );
}

function RequiredMark() {
  return (
    <>
      <span className="ml-0.5 text-destructive" aria-hidden="true">*</span>
      <span className="sr-only"> (required)</span>
    </>
  );
}

function FieldInput({
  field,
  value,
  onChange,
  error,
  missing,
  disabled,
}: {
  field: ContentField;
  value: unknown;
  onChange: (value: unknown) => void;
  error: string | null;
  missing: boolean;
  disabled: boolean;
}) {
  const id = `field-${field.key}`;
  const helpId = `${id}-help`;
  const errorId = `${id}-error`;
  const describedBy = [field.help !== "" ? helpId : null, error !== null ? errorId : null].filter((x) => x !== null).join(" ") || undefined;
  const common = {
    id,
    disabled,
    "aria-invalid": error !== null ? true : undefined,
    "aria-describedby": describedBy,
  };
  const wide = field.kind === "textarea" || field.kind === "media" || field.kind === "entry" || (field.kind === "choice" && field.options.multiple === true);
  const text = typeof value === "string" ? value : "";
  // Media and entry fields have no single input: their buttons name the
  // field themselves ("Change Photo"), so the label is just a heading.
  const picker = field.kind === "media" || field.kind === "entry";
  const label = picker ? (
    <p className="block text-sm font-medium">
      {field.label}
      {field.required ? <RequiredMark /> : null}
    </p>
  ) : (
    <label htmlFor={id} className="block text-sm font-medium">
      {field.label}
      {field.required ? <RequiredMark /> : null}
    </label>
  );

  let input: React.ReactNode;
  switch (field.kind) {
    case "text":
      input = <Input {...common} type="text" value={text} maxLength={field.options.max_length ?? 255} onChange={(e) => onChange(e.target.value === "" ? undefined : e.target.value)} />;
      break;
    case "url":
      input = <Input {...common} type="text" inputMode="url" placeholder="https://… or /a-path-on-this-site" value={text} onChange={(e) => onChange(e.target.value.trim() === "" ? undefined : e.target.value.trim())} />;
      break;
    case "textarea":
      input = (
        <textarea
          {...common}
          rows={4}
          value={text}
          maxLength={field.options.max_length ?? 10000}
          onChange={(e) => onChange(e.target.value === "" ? undefined : e.target.value)}
          className="w-full resize-y rounded-md border border-input bg-background p-2 text-sm"
        />
      );
      break;
    case "number":
      input = <NumberInput common={common} field={field} value={value} onChange={onChange} />;
      break;
    case "date":
      input = <Input {...common} type="date" value={text} onChange={(e) => onChange(e.target.value === "" ? undefined : e.target.value)} />;
      break;
    case "boolean":
      // A checkbox labels itself; the field label above it would be a
      // second label for the same control.
      return (
        <div data-field={field.key} className="space-y-1.5">
          <label className="flex items-start gap-2 text-sm font-medium">
            <input
              {...common}
              type="checkbox"
              checked={value === true}
              onChange={(e) => onChange(e.target.checked)}
              className="mt-0.5 h-4 w-4 accent-primary"
            />
            <span>
              {field.label}
              {field.required ? <RequiredMark /> : null}
            </span>
          </label>
          <Notes field={field} helpId={helpId} errorId={errorId} error={error} missing={false} />
        </div>
      );
    case "choice":
      if (field.options.multiple === true) {
        const picked = Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
        return (
          <fieldset data-field={field.key} className={cn("space-y-1.5", wide && "sm:col-span-2")} aria-describedby={describedBy}>
            <legend className="text-sm font-medium">
              {field.label}
              {field.required ? <RequiredMark /> : null}
            </legend>
            <div className="flex flex-wrap gap-x-4 gap-y-1">
              {(field.options.choices ?? []).map((c) => (
                <label key={c} className="flex items-center gap-1.5 text-sm">
                  <input
                    type="checkbox"
                    disabled={disabled}
                    checked={picked.includes(c)}
                    onChange={(e) => {
                      const next = new Set(picked);
                      if (e.target.checked) next.add(c);
                      else next.delete(c);
                      // Kept in the order the choices are listed.
                      const list = (field.options.choices ?? []).filter((x) => next.has(x));
                      onChange(list.length === 0 ? undefined : list);
                    }}
                    className="h-3.5 w-3.5 accent-primary"
                  />
                  {c}
                </label>
              ))}
            </div>
            <Notes field={field} helpId={helpId} errorId={errorId} error={error} missing={false} />
          </fieldset>
        );
      }
      input = (
        <select
          {...common}
          value={text}
          onChange={(e) => onChange(e.target.value === "" ? undefined : e.target.value)}
          className="h-9 w-full rounded-md border border-input bg-background px-2 text-sm"
        >
          <option value="">—</option>
          {(field.options.choices ?? []).map((c) => <option key={c} value={c}>{c}</option>)}
          {text !== "" && !(field.options.choices ?? []).includes(text) ? <option value={text}>{text} (no longer a choice)</option> : null}
        </select>
      );
      break;
    case "media":
      input = <MediaValue common={common} field={field} id={idOf(value)} onChange={onChange} />;
      break;
    case "entry":
      input = <EntryValue common={common} field={field} id={idOf(value)} onChange={onChange} />;
      break;
    default:
      input = <p className="text-xs text-muted-foreground">This kind of field can&rsquo;t be edited here.</p>;
  }

  return (
    <div data-field={field.key} className={cn("space-y-1.5", wide && "sm:col-span-2")}>
      {label}
      {input}
      <Notes field={field} helpId={helpId} errorId={errorId} error={error} missing={missing} />
    </div>
  );
}

function Notes({ field, helpId, errorId, error, missing }: { field: ContentField; helpId: string; errorId: string; error: string | null; missing: boolean }) {
  return (
    <>
      {missing ? (
        <p className="flex items-start gap-1 text-xs text-warning">
          <AlertTriangle className="mt-px h-3.5 w-3.5 shrink-0" aria-hidden="true" />
          {field.kind === "media"
            ? "The media item this points to no longer exists (deleted or in the trash). The site shows nothing for it; pick another or remove it."
            : "The entry this points to no longer exists (deleted or in the trash). The site shows nothing for it; pick another or remove it."}
        </p>
      ) : null}
      {field.help !== "" ? <p id={helpId} className="text-xs text-muted-foreground">{field.help}</p> : null}
      {error !== null ? <p id={errorId} role="alert" className="text-xs text-destructive">{error}</p> : null}
    </>
  );
}

/** Media and entry ids arrive as strings (64-bit); an older value may be a number. */
function idOf(value: unknown): string | null {
  if (typeof value === "string" && value !== "") return value;
  if (typeof value === "number") return String(value);
  return null;
}

type Common = { id: string; disabled: boolean; "aria-invalid"?: boolean; "aria-describedby"?: string };

/**
 * A number input that lets a half-typed number ("12.", "-") stay on screen:
 * the value only changes when what is typed is a number, or is cleared.
 */
function NumberInput({ common, field, value, onChange }: { common: Common; field: ContentField; value: unknown; onChange: (v: unknown) => void }) {
  const outside = typeof value === "number" ? String(value) : "";
  const [draft, setDraft] = React.useState(outside);
  React.useEffect(() => {
    setDraft((d) => (d.trim() !== "" && Number(d) === value ? d : outside));
  }, [outside, value]);
  return (
    <Input
      {...common}
      type="number"
      inputMode="decimal"
      min={field.options.min}
      max={field.options.max}
      step={field.options.step ?? "any"}
      value={draft}
      onChange={(e) => {
        const next = e.target.value;
        setDraft(next);
        if (next.trim() === "") onChange(undefined);
        else if (Number.isFinite(Number(next))) onChange(Number(next));
      }}
    />
  );
}

function MediaValue({ common, field, id, onChange }: { common: Common; field: ContentField; id: string | null; onChange: (v: unknown) => void }) {
  const [picking, setPicking] = React.useState(false);
  const media = useQuery({
    queryKey: ["media", "one", id],
    queryFn: () => api.getMedia(id as string),
    enabled: id !== null,
    retry: false,
    staleTime: 60_000,
  });
  const m = media.data;
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        {id === null ? (
          <span className="text-sm text-muted-foreground">Nothing chosen</span>
        ) : (
          <span className="flex min-w-0 items-center gap-2">
            {m !== undefined && m.mime.startsWith("image/") ? (
              <img src={`/api/v1/media/${id}/raw`} alt="" className="h-12 w-12 shrink-0 rounded border object-cover" />
            ) : (
              <FileText className="h-5 w-5 shrink-0 text-muted-foreground" aria-hidden="true" />
            )}
            <span className="truncate text-sm">{m?.file_name ?? `Media #${id}`}</span>
          </span>
        )}
        <Button type="button" size="sm" variant="outline" className="ml-auto h-8" disabled={common.disabled} aria-label={`${picking ? "Close" : id === null ? "Choose" : "Change"} ${field.label}`} aria-describedby={common["aria-describedby"]} aria-invalid={common["aria-invalid"]} onClick={() => setPicking((v) => !v)}>
          {picking ? "Close" : id === null ? "Choose" : "Change"}
        </Button>
        {id !== null ? (
          <Button type="button" size="sm" variant="ghost" className="h-8 w-8 p-0" disabled={common.disabled} aria-label={`Remove ${field.label}`} onClick={() => onChange(undefined)}>
            <X className="h-4 w-4" aria-hidden="true" />
          </Button>
        ) : null}
      </div>
      {picking ? (
        <MediaPicker
          accept="any"
          onPick={(picked) => {
            onChange(String(picked.id));
            setPicking(false);
          }}
        />
      ) : null}
    </div>
  );
}

function EntryValue({ common, field, id, onChange }: { common: Common; field: ContentField; id: string | null; onChange: (v: unknown) => void }) {
  const [picking, setPicking] = React.useState(false);
  const [term, setTerm] = React.useState("");
  const [debounced, setDebounced] = React.useState("");
  React.useEffect(() => {
    const t = setTimeout(() => setDebounced(term), 250);
    return () => clearTimeout(t);
  }, [term]);
  const current = useQuery({
    queryKey: ["entry-ref", id],
    queryFn: () => api.getPost(id as string),
    enabled: id !== null,
    retry: false,
    staleTime: 60_000,
  });
  const entryType = field.options.entry_type;
  const results = useQuery({
    queryKey: ["entry-picker", entryType ?? "", debounced],
    queryFn: () => api.listPosts({ ...(entryType === undefined ? {} : { type: entryType }), ...(debounced === "" ? {} : { search: debounced }), per_page: 8 }),
    enabled: picking,
    staleTime: 30_000,
  });
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        {id === null ? (
          <span className="text-sm text-muted-foreground">Nothing chosen</span>
        ) : (
          <span className="min-w-0 truncate text-sm">
            {current.data !== undefined ? current.data.title || "(untitled)" : `Entry #${id}`}
            {current.data !== undefined ? <span className="ml-1.5 text-xs text-muted-foreground">{current.data.type} · {current.data.status}</span> : null}
          </span>
        )}
        <Button type="button" size="sm" variant="outline" className="ml-auto h-8" disabled={common.disabled} aria-label={`${picking ? "Close" : id === null ? "Choose" : "Change"} ${field.label}`} aria-describedby={common["aria-describedby"]} aria-invalid={common["aria-invalid"]} onClick={() => setPicking((v) => !v)}>
          {picking ? "Close" : id === null ? "Choose" : "Change"}
        </Button>
        {id !== null ? (
          <Button type="button" size="sm" variant="ghost" className="h-8 w-8 p-0" disabled={common.disabled} aria-label={`Remove ${field.label}`} onClick={() => onChange(undefined)}>
            <X className="h-4 w-4" aria-hidden="true" />
          </Button>
        ) : null}
      </div>
      {picking ? (
        <div className="space-y-1.5 rounded-md border p-2">
          <div className="relative">
            <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
            <Input autoFocus aria-label={`Search for ${field.label}`} value={term} onChange={(e) => setTerm(e.target.value)} placeholder="Search by title" className="pl-7" />
          </div>
          <ul className="max-h-48 overflow-y-auto text-sm">
            {(results.data?.items ?? []).map((p) => (
              <li key={p.id}>
                <button
                  type="button"
                  className="flex w-full items-baseline gap-2 rounded px-2 py-1 text-left hover:bg-accent"
                  onClick={() => {
                    onChange(String(p.id));
                    setPicking(false);
                    setTerm("");
                  }}
                >
                  <span className="min-w-0 flex-1 truncate">{p.title || "(untitled)"}</span>
                  <span className="shrink-0 text-xs text-muted-foreground">{p.type} · {p.status}</span>
                </button>
              </li>
            ))}
            {results.isSuccess && (results.data?.items ?? []).length === 0 ? <li className="px-2 py-1 text-xs text-muted-foreground">Nothing found.</li> : null}
          </ul>
        </div>
      ) : null}
    </div>
  );
}
