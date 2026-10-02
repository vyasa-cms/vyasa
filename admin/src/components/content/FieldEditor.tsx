import * as React from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowLeft, ArrowUp, ListPlus, Plus } from "lucide-react";
import { api, type ContentField, type FieldKind, type FieldOptions } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, EmptyState, ErrorNote, Field, PageHeader, Panel } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { Input } from "@/components/ui/input";
import { errorText } from "@/lib/error-text";
import { FIELD_KEY_RE, FIELD_KINDS, keyFromLabel, kindLabel } from "@/lib/fields";
import { cn } from "@/lib/utils";

/**
 * The fields of one content type: add, order, edit and remove them, and
 * clean up the values a removed field left in entries.
 *
 * Removing a field keeps what entries stored (it is no longer shown or
 * served); "Clean up" removes it for good. A key whose values are still
 * stored cannot be reused until then, so old values never pass for a new
 * field's.
 */
export function FieldEditor({ slug }: { slug: string }) {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [adding, setAdding] = React.useState(false);
  const [editing, setEditing] = React.useState<ContentField | null>(null);

  const types = useQuery({ queryKey: ["content-types"], queryFn: () => api.listContentTypes() });
  const type = types.data?.find((t) => t.slug === slug);
  const fields = useQuery({ queryKey: ["content-fields", slug], queryFn: () => api.listFields(slug) });
  const orphans = useQuery({ queryKey: ["content-orphans", slug], queryFn: () => api.listOrphans(slug) });
  const list = fields.data ?? [];

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ["content-fields", slug] });
    void queryClient.invalidateQueries({ queryKey: ["content-orphans", slug] });
  };

  const reorder = useMutation({
    mutationFn: (keys: string[]) => api.reorderFields(slug, keys),
    onSuccess: (next) => queryClient.setQueryData(["content-fields", slug], next),
    onError: (e) => notify.error("Couldn't reorder the fields", e),
  });
  const move = (index: number, by: -1 | 1) => {
    const keys = list.map((f) => f.key);
    const [key] = keys.splice(index, 1);
    if (key === undefined) return;
    keys.splice(index + by, 0, key);
    reorder.mutate(keys);
  };

  const remove = useMutation({
    mutationFn: (f: ContentField) => api.deleteField(slug, f.key),
    onSuccess: (_r, f) => {
      refresh();
      notify.success(`${f.label} removed`, "Values entries hold for it are kept until you clean them up.");
    },
    onError: (e) => notify.error("Couldn't delete the field", e),
  });
  const askRemove = async (f: ContentField) => {
    const ok = await confirm({
      title: `Delete ${f.label}?`,
      description:
        "Stored values are kept in the entries, but no longer shown or served. Remove them for good with “Clean up” under Left-over values; until then the key can’t be used for a new field.",
      confirmLabel: "Delete field",
      destructive: true,
    });
    if (ok) remove.mutate(f);
  };

  const cleanUp = useMutation({
    mutationFn: (key: string) => api.cleanUpOrphan(slug, key),
    onSuccess: (r) => {
      refresh();
      notify.success("Values removed", `From ${r.entries} ${r.entries === 1 ? "entry" : "entries"}.`);
    },
    onError: (e) => notify.error("Couldn't remove the values", e),
  });
  const askCleanUp = async (key: string, entries: number) => {
    const ok = await confirm({
      title: `Remove the values stored under ${key}?`,
      description: `${entries} ${entries === 1 ? "entry holds" : "entries hold"} a value no field shows any more. Removing it can't be undone; revisions keep their own copies.`,
      confirmLabel: "Remove values",
      destructive: true,
    });
    if (ok) cleanUp.mutate(key);
  };

  const name = type?.plural ?? slug;
  const left = orphans.data ?? [];

  return (
    <div className="space-y-4 sm:space-y-5">
      <Link to="/content-types" className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
        <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" />
        Content types
      </Link>
      <PageHeader
        title={`Fields of ${name}`}
        description={
          <>
            Extra values every {type?.singular.toLowerCase() ?? "entry"} carries, edited in the editor&rsquo;s Fields panel and shown by templates as{" "}
            <code className="font-mono text-xs">entry.fields.&lt;key&gt;</code>.
          </>
        }
        actions={<Button size="sm" onClick={() => setAdding(true)}><Plus className="h-4 w-4" aria-hidden="true" />Add field</Button>}
      />

      {fields.isError ? <ErrorNote title="Couldn't load the fields" error={fields.error} onRetry={() => void fields.refetch()} /> : null}

      {fields.isSuccess && list.length === 0 ? (
        <EmptyState icon={ListPlus} title="No fields yet" description="Add one for each extra value this type needs — a price, a date, a link to another entry." />
      ) : (
        <ul className="divide-y rounded-lg border bg-card" data-testid="fields">
          {list.map((f, i) => (
            <li key={f.key} className="flex flex-wrap items-center gap-3 px-3 py-2.5">
              <span className="min-w-0 flex-1">
                <span className="flex flex-wrap items-center gap-1.5">
                  <span className="truncate font-medium" data-testid="field-label">{f.label}</span>
                  <Chip tone="info" dot={false}>{kindLabel(f.kind)}</Chip>
                  {f.required ? <Chip tone="warning" dot={false}>Required</Chip> : null}
                </span>
                <span className="block truncate font-mono text-[11px] text-muted-foreground">{f.key}</span>
                {f.help !== "" ? <span className="block truncate text-xs text-muted-foreground">{f.help}</span> : null}
              </span>
              <span className="flex items-center gap-0.5">
                <Button variant="ghost" size="sm" className="h-7 w-7 p-0" aria-label={`Move ${f.label} up`} disabled={i === 0 || reorder.isPending} onClick={() => move(i, -1)}>
                  <ArrowUp className="h-3.5 w-3.5" aria-hidden="true" />
                </Button>
                <Button variant="ghost" size="sm" className="h-7 w-7 p-0" aria-label={`Move ${f.label} down`} disabled={i === list.length - 1 || reorder.isPending} onClick={() => move(i, 1)}>
                  <ArrowDown className="h-3.5 w-3.5" aria-hidden="true" />
                </Button>
                <button type="button" onClick={() => setEditing(f)} className="rounded-md px-2 py-1 text-xs font-medium text-muted-foreground hover:bg-accent hover:text-foreground">Edit</button>
                <button type="button" onClick={() => void askRemove(f)} className="rounded-md px-2 py-1 text-xs font-medium text-destructive hover:bg-destructive-subtle">Delete</button>
              </span>
            </li>
          ))}
        </ul>
      )}

      {left.length > 0 ? (
        <Panel testId="orphans" title="Left-over values" description="Entries still hold values for fields that were deleted. They are not shown or served; remove them when you no longer need them.">
          <ul className="divide-y rounded-md border text-sm">
            {left.map((o) => (
              <li key={o.key} className="flex items-center gap-3 px-3 py-2">
                <span className="min-w-0 flex-1">
                  <span className="font-mono text-xs">{o.key}</span>
                  <span className="ml-2 text-xs text-muted-foreground">{o.entries} {o.entries === 1 ? "entry" : "entries"}</span>
                </span>
                <Button size="sm" variant="outline" className="h-7 text-xs" aria-label={`Clean up ${o.key}`} disabled={cleanUp.isPending} onClick={() => void askCleanUp(o.key, o.entries)}>
                  Clean up
                </Button>
              </li>
            ))}
          </ul>
        </Panel>
      ) : null}

      {adding || editing !== null ? (
        // Mounted fresh for each opening, so the form starts from what it
        // edits before the first keystroke lands.
        <FieldForm
          key={editing?.key ?? "new"}
          slug={slug}
          editing={editing}
          onClose={() => { setAdding(false); setEditing(null); }}
          onSaved={refresh}
        />
      ) : null}
    </div>
  );
}

interface FormState {
  label: string;
  key: string;
  keyTouched: boolean;
  kind: FieldKind;
  help: string;
  required: boolean;
  maxLength: string;
  min: string;
  max: string;
  step: string;
  choices: string;
  multiple: boolean;
  entryType: string;
}

const EMPTY: FormState = {
  label: "", key: "", keyTouched: false, kind: "text", help: "", required: false,
  maxLength: "", min: "", max: "", step: "", choices: "", multiple: false, entryType: "",
};

function stateOf(f: ContentField): FormState {
  const num = (n: number | undefined) => (n === undefined ? "" : String(n));
  return {
    label: f.label,
    key: f.key,
    keyTouched: true,
    kind: f.kind,
    help: f.help,
    required: f.required,
    maxLength: num(f.options.max_length),
    min: num(f.options.min),
    max: num(f.options.max),
    step: num(f.options.step),
    choices: (f.options.choices ?? []).join("\n"),
    multiple: f.options.multiple === true,
    entryType: f.options.entry_type ?? "",
  };
}

/** A number typed into an option box, or nothing when it is blank or not a number. */
function numberOf(text: string): number | undefined {
  if (text.trim() === "") return undefined;
  const n = Number(text);
  return Number.isFinite(n) ? n : undefined;
}

/** Only the options the chosen kind takes: `options` replaces the stored ones. */
function optionsOf(v: FormState): FieldOptions {
  const out: FieldOptions = {};
  const put = (key: "max_length" | "min" | "max" | "step", text: string) => {
    const n = numberOf(text);
    if (n !== undefined) out[key] = n;
  };
  switch (v.kind) {
    case "text":
    case "textarea":
      put("max_length", v.maxLength);
      break;
    case "number":
      put("min", v.min);
      put("max", v.max);
      put("step", v.step);
      break;
    case "choice":
      out.choices = v.choices.split("\n").map((c) => c.trim()).filter((c) => c !== "");
      out.multiple = v.multiple;
      break;
    case "entry":
      if (v.entryType !== "") out.entry_type = v.entryType;
      break;
    default:
      break;
  }
  return out;
}

function FieldForm({
  slug,
  editing,
  onClose,
  onSaved,
}: {
  slug: string;
  editing: ContentField | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [v, setV] = React.useState<FormState>(() => (editing === null ? EMPTY : stateOf(editing)));
  const [failure, setFailure] = React.useState<string | null>(null);
  const types = useQuery({ queryKey: ["content-types"], queryFn: () => api.listContentTypes() });

  const key = v.key.trim();
  const label = v.label.trim();
  const keyError = editing !== null || key === "" || FIELD_KEY_RE.test(key) ? null : "Start with a letter; then lowercase letters, digits and _ (up to 40).";
  const numberError = (text: string) => (text.trim() !== "" && numberOf(text) === undefined ? "Enter a number." : null);
  const choices = optionsOf(v).choices ?? [];
  const valid =
    label !== "" &&
    (editing !== null || FIELD_KEY_RE.test(key)) &&
    (v.kind !== "choice" || choices.length > 0) &&
    [v.maxLength, v.min, v.max, v.step].every((t) => numberError(t) === null);

  const save = useMutation({
    mutationFn: () => {
      const body = { label, help: v.help.trim(), kind: v.kind, required: v.required, options: optionsOf(v) };
      return editing === null ? api.createField(slug, { key, ...body }) : api.updateField(slug, editing.key, body);
    },
    onSuccess: (f) => {
      notify.success(editing === null ? `${f.label} added` : `${f.label} saved`);
      onSaved();
      onClose();
    },
    onError: (e) => {
      setFailure(errorText(e));
      notify.error(editing === null ? "Couldn't add the field" : "Couldn't save the field", e);
    },
  });

  const set = (patch: Partial<FormState>) => setV((prev) => ({ ...prev, ...patch }));
  const kindHint = FIELD_KINDS.find((k) => k.kind === v.kind)?.hint;

  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      title={editing === null ? "Add a field" : `Edit ${editing.label}`}
      description={editing === null ? "One extra value for every entry of this type." : "Changing the kind is refused while entries hold a value for this field."}
      footer={
        <>
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          <Button disabled={!valid || save.isPending} onClick={() => save.mutate()}>
            {save.isPending ? "Saving…" : editing === null ? "Create field" : "Save field"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {failure !== null ? (
          <p role="alert" className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">{failure}</p>
        ) : null}
        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="Label" htmlFor="field-label">
            <Input
              id="field-label"
              autoFocus
              value={v.label}
              onChange={(e) => set({ label: e.target.value, ...(v.keyTouched ? {} : { key: keyFromLabel(e.target.value) }) })}
              placeholder="Price"
            />
          </Field>
          <Field
            label="Key"
            htmlFor="field-key"
            hint={editing === null ? "How entries store it and templates read it (fields.key). Can't be changed later." : "The key can't change."}
            error={keyError}
          >
            <Input id="field-key" disabled={editing !== null} value={v.key} onChange={(e) => set({ key: e.target.value, keyTouched: true })} className="font-mono text-sm" />
          </Field>
        </div>
        <Field label="Kind" htmlFor="field-kind" hint={kindHint}>
          <select id="field-kind" value={v.kind} onChange={(e) => set({ kind: e.target.value as FieldKind })} className="h-9 w-full rounded-md border border-input bg-background px-2 text-sm">
            {FIELD_KINDS.map((k) => <option key={k.kind} value={k.kind}>{k.label}</option>)}
          </select>
        </Field>

        {v.kind === "text" || v.kind === "textarea" ? (
          <Field label="Maximum length" htmlFor="field-max-length" hint={v.kind === "text" ? "Characters; 1 to 1000 (255 when blank)." : "Characters; 1 to 100000 (10000 when blank)."} error={numberError(v.maxLength)}>
            <Input id="field-max-length" inputMode="numeric" value={v.maxLength} onChange={(e) => set({ maxLength: e.target.value })} />
          </Field>
        ) : null}
        {v.kind === "number" ? (
          <div className="grid gap-4 sm:grid-cols-3">
            <Field label="Minimum" htmlFor="field-min" error={numberError(v.min)}>
              <Input id="field-min" inputMode="decimal" value={v.min} onChange={(e) => set({ min: e.target.value })} />
            </Field>
            <Field label="Maximum" htmlFor="field-max" error={numberError(v.max)}>
              <Input id="field-max" inputMode="decimal" value={v.max} onChange={(e) => set({ max: e.target.value })} />
            </Field>
            <Field label="Step" htmlFor="field-step" hint="e.g. 0.01 for prices" error={numberError(v.step)}>
              <Input id="field-step" inputMode="decimal" value={v.step} onChange={(e) => set({ step: e.target.value })} />
            </Field>
          </div>
        ) : null}
        {v.kind === "choice" ? (
          <>
            <Field label="Choices" htmlFor="field-choices" hint="One per line; up to 100, each different.">
              <textarea id="field-choices" rows={4} value={v.choices} onChange={(e) => set({ choices: e.target.value })} className="w-full resize-y rounded-md border border-input bg-background p-2 text-sm" />
            </Field>
            <label className="flex items-start gap-2 text-sm">
              <input type="checkbox" checked={v.multiple} onChange={(e) => set({ multiple: e.target.checked })} className="mt-0.5 h-3.5 w-3.5 accent-primary" />
              <span>Allow picking more than one</span>
            </label>
          </>
        ) : null}
        {v.kind === "entry" ? (
          <Field label="Points at" htmlFor="field-entry-type" hint="Only entries of this type can be picked.">
            <select id="field-entry-type" value={v.entryType} onChange={(e) => set({ entryType: e.target.value })} className="h-9 w-full rounded-md border border-input bg-background px-2 text-sm">
              <option value="">Any type</option>
              {(types.data ?? []).map((t) => <option key={t.slug} value={t.slug}>{t.plural}</option>)}
            </select>
          </Field>
        ) : null}

        <Field label="Help text" htmlFor="field-help" hint="Optional. Shown under the field in the editor.">
          <Input id="field-help" value={v.help} onChange={(e) => set({ help: e.target.value })} />
        </Field>
        <label className={cn("flex items-start gap-2 text-sm")}>
          <input type="checkbox" checked={v.required} onChange={(e) => set({ required: e.target.checked })} className="mt-0.5 h-3.5 w-3.5 accent-primary" />
          <span>Required — needed to publish or schedule; drafts save without it</span>
        </label>
      </div>
    </Modal>
  );
}
