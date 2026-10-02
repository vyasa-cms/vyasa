import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ClipboardList, Plus } from "lucide-react";
import { api, type FormDef, type FormField, type FormSubmission } from "@/api/client";
import { Button } from "@/components/ui/button";
import { DataList, type Column } from "@/components/ui/data-list";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Chip, EmptyState, ErrorNote, Field, PageHeader } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";

export const Route = createFileRoute("/_auth/forms/")({ component: FormsPage });

const KINDS: { value: FormField["kind"]; label: string }[] = [
  { value: "text", label: "Short text" },
  { value: "email", label: "Email" },
  { value: "textarea", label: "Long text" },
  { value: "number", label: "Number" },
  { value: "select", label: "Choice" },
  { value: "checkbox", label: "Checkbox" },
];

const NEW_FORM: FormField[] = [
  { key: "name", label: "Your name", kind: "text", required: true, options: [] },
  { key: "email", label: "Email", kind: "email", required: true, options: [] },
  { key: "message", label: "Message", kind: "textarea", required: true, options: [] },
];

/**
 * Forms an editor designs. Each has fields, an address to notify, and an
 * inbox. Put one on a page with the Form block and its slug.
 */
export function FormsPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const forms = useQuery({ queryKey: ["forms"], queryFn: () => api.listForms() });
  const [editing, setEditing] = React.useState<FormDef | "new" | null>(null);
  const [inbox, setInbox] = React.useState<FormDef | null>(null);
  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["forms"] });
  const remove = useMutation({
    mutationFn: (f: FormDef) => api.deleteForm(f.id),
    onSuccess: () => { invalidate(); notify.success("Form removed", "Its submissions are kept under Audience."); },
    onError: (e) => notify.error("Couldn't remove the form", e),
  });
  const rows = forms.data ?? [];
  const columns: Column<FormDef>[] = [
    { key: "name", header: "Form", primary: true, render: (f) => <span className="min-w-0"><button type="button" className="block truncate font-medium underline-offset-2 hover:underline" onClick={() => setInbox(f)}>{f.name}</button><span className="block font-mono text-[11px] text-muted-foreground">{f.slug} · {f.fields.length} {f.fields.length === 1 ? "field" : "fields"}{f.notify_email ? ` · notifies ${f.notify_email}` : ""}</span></span> },
    { key: "state", header: "State", width: "8rem", render: (f) => <Chip tone={f.enabled ? "success" : "neutral"} dot={false}>{f.enabled ? "Live" : "Off"}</Chip> },
    { key: "unread", header: "Unread", width: "7rem", render: (f) => f.unread > 0 ? <Chip tone="info" dot={false}>{f.unread} new</Chip> : <span className="text-xs text-muted-foreground">none</span> },
  ];
  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader title="Forms" description="Design a form, drop it on any page with the Form block, and read the answers here." actions={<Button size="sm" onClick={() => setEditing("new")}><Plus className="h-4 w-4" aria-hidden="true" />New form</Button>} />
      <DataList
        testId="forms-table"
        rows={rows}
        columns={columns}
        rowKey={(f) => f.id}
        isLoading={forms.isPending}
        error={forms.error}
        rowActions={(f) => (
          <span className="flex gap-0.5">
            <Button size="sm" variant="ghost" className="h-7 text-xs" onClick={() => setInbox(f)}>Inbox</Button>
            <Button size="sm" variant="ghost" className="h-7 text-xs" onClick={() => setEditing(f)}>Edit</Button>
            <Button size="sm" variant="ghost" className="h-7 text-xs text-destructive" onClick={async () => { if (await confirm({ title: `Remove "${f.name}"?`, description: "Pages that embed it show nothing where it stood. Submissions already received are kept.", confirmLabel: "Remove", destructive: true })) remove.mutate(f); }}>Remove</Button>
          </span>
        )}
        empty={<EmptyState icon={ClipboardList} title="No forms yet" description="Make one, then add a Form block to a page and give it the form's slug." />}
      />
      {editing ? <FormEditor form={editing === "new" ? null : editing} onClose={() => setEditing(null)} onSaved={invalidate} /> : null}
      {inbox ? <Inbox form={inbox} onClose={() => setInbox(null)} onRead={invalidate} /> : null}
    </div>
  );
}

function FormEditor({ form, onClose, onSaved }: { form: FormDef | null; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = React.useState(form?.name ?? "");
  const [notify_email, setNotify] = React.useState(form?.notify_email ?? "");
  const [success, setSuccess] = React.useState(form?.success_message ?? "");
  const [enabled, setEnabled] = React.useState(form?.enabled ?? true);
  const [choices, setChoices] = React.useState<Record<string, string>>({});
  const [fields, setFields] = React.useState<FormField[]>(form?.fields ?? NEW_FORM);
  const save = useMutation({
    mutationFn: () => {
      const body = { name: name.trim(), fields: fields.map((f, i) => ({ ...f, options: choices[i] === undefined ? f.options : choices[i].split(",").map(s => s.trim()).filter(Boolean) })), notify_email: notify_email.trim(), success_message: success, enabled };
      return form ? api.updateForm(form.id, body) : api.createForm(body);
    },
    onSuccess: (f) => { onSaved(); onClose(); notify.success(form ? "Form saved" : "Form created", `Add a Form block with the slug ${f.slug}.`); },
    onError: (e) => notify.error("Couldn't save the form", e),
  });
  const setField = (i: number, patch: Partial<FormField>) => setFields((fs) => fs.map((f, j) => (j === i ? { ...f, ...patch } : f)));
  const move = (i: number, d: -1 | 1) => setFields((fs) => { const n = [...fs]; const j = i + d; if (j < 0 || j >= n.length) return fs; [n[i], n[j]] = [n[j]!, n[i]!]; return n; });
  return (
    <Modal open onClose={onClose} size="lg" title={form ? `Edit "${form.name}"` : "New form"} description="Field keys become input names; keep them simple." testId="form-editor"
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button disabled={name.trim() === "" || fields.length === 0 || save.isPending} onClick={() => save.mutate()}>{save.isPending ? "Saving…" : form ? "Save" : "Create"}</Button></>}>
      <div className="space-y-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Name" htmlFor="fm-name"><Input id="fm-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Contact us" /></Field>
          <Field label="Send answers to" htmlFor="fm-notify" hint="Empty: answers only appear in the inbox."><Input id="fm-notify" type="email" value={notify_email} onChange={(e) => setNotify(e.target.value)} placeholder="hello@yourdomain.com" /></Field>
          <div className="sm:col-span-2"><Field label="Message after sending" htmlFor="fm-ok"><Input id="fm-ok" value={success} onChange={(e) => setSuccess(e.target.value)} placeholder="Thanks — your message has been received." /></Field></div>
        </div>
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" className="accent-primary" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />Live: the form accepts answers</label>
        <div>
          <h3 className="mb-1 text-sm font-medium">Fields</h3>
          <ul className="space-y-2" data-testid="form-fields">
            {fields.map((f, i) => (
              <li key={i} className="grid gap-2 rounded-md border p-2 sm:grid-cols-[1fr_1fr_9rem_auto]">
                <Input aria-label="Field label" value={f.label} onChange={(e) => setField(i, { label: e.target.value })} placeholder="Label" />
                <Input aria-label="Field key" value={f.key} onChange={(e) => setField(i, { key: e.target.value.replace(/[^a-zA-Z0-9_-]/g, "") })} placeholder="key" className="font-mono text-sm" />
                <select aria-label="Field kind" value={f.kind} onChange={(e) => setField(i, { kind: e.target.value as FormField["kind"] })} className="h-9 rounded-md border bg-background px-2 text-sm">{KINDS.map((k) => <option key={k.value} value={k.value}>{k.label}</option>)}</select>
                <span className="flex items-center gap-1">
                  <label className="flex items-center gap-1 text-xs"><input type="checkbox" className="accent-primary" checked={f.required} onChange={(e) => setField(i, { required: e.target.checked })} />Required</label>
                  <button type="button" aria-label="Move up" className="px-1 text-muted-foreground" onClick={() => move(i, -1)}>↑</button>
                  <button type="button" aria-label="Move down" className="px-1 text-muted-foreground" onClick={() => move(i, 1)}>↓</button>
                  <button type="button" aria-label="Remove field" className="px-1 text-destructive" onClick={() => setFields((fs) => fs.filter((_, j) => j !== i))}>×</button>
                </span>
                {f.kind === "select" ? <div className="sm:col-span-4"><Input aria-label="Choices" value={choices[i] ?? f.options.join(", ")} onChange={(e) => setChoices(c => ({ ...c, [i]: e.target.value }))} onBlur={() => { if (choices[i] !== undefined) setField(i, { options: choices[i].split(",").map(s => s.trim()).filter(Boolean) }); setChoices({}); }} placeholder="Choices, separated by commas" /></div> : null}
              </li>
            ))}
          </ul>
          <Button size="sm" variant="outline" className="mt-2" onClick={() => setFields((fs) => [...fs, { key: `field_${fs.length + 1}`, label: "", kind: "text", required: false, options: [] }])}>Add field</Button>
        </div>
      </div>
    </Modal>
  );
}

function Inbox({ form, onClose, onRead }: { form: FormDef; onClose: () => void; onRead: () => void }) {
  const queryClient = useQueryClient();
  const list = useQuery({ queryKey: ["form-submissions", form.id], queryFn: () => api.formSubmissions(form.id) });
  const read = useMutation({
    mutationFn: (ids: string[]) => api.markSubmissionsRead(ids),
    onSuccess: () => { void queryClient.invalidateQueries({ queryKey: ["form-submissions", form.id] }); onRead(); },
    onError: (e) => notify.error("Couldn't mark submissions read", e),
  });
  const rows = list.data ?? [];
  const unread = rows.filter((r) => r.read_at === null).map((r) => r.id);
  return (
    <Modal open onClose={onClose} size="lg" title={`${form.name}: inbox`} description={`${rows.length} ${rows.length === 1 ? "answer" : "answers"}`} testId="form-inbox"
      footer={<><a href={`/api/v1/forms/${form.id}/submissions.csv`} className="inline-flex h-9 items-center rounded-md border px-3 text-sm hover:bg-accent" download>Download CSV</a><span className="flex-1" />{unread.length > 0 ? <Button variant="outline" onClick={() => read.mutate(unread)}>Mark all read</Button> : null}<Button onClick={onClose}>Close</Button></>}>
      {list.isPending ? <p role="status">Loading answers…</p> : list.isError ? <ErrorNote title="Couldn't load answers" error={list.error} onRetry={() => void list.refetch()} /> : rows.length === 0 ? <p className="text-sm text-muted-foreground">Nothing yet.</p> : (
        <ul className="divide-y rounded-md border text-sm">
          {rows.map((r: FormSubmission) => (
            <li key={r.id} className={r.read_at === null ? "bg-primary/5 px-3 py-2" : "px-3 py-2"}>
              <div className="flex flex-wrap items-baseline gap-2 text-xs text-muted-foreground">
                <span>{new Date(r.created_at).toLocaleString()}</span>
                {r.path ? <span>from {r.path}</span> : null}
                {r.read_at === null ? <button type="button" className="ml-auto text-primary underline-offset-2 hover:underline" onClick={() => read.mutate([r.id])}>Mark read</button> : null}
              </div>
              <dl className="mt-1 grid gap-x-4 gap-y-0.5 sm:grid-cols-[auto_1fr]">
                {[...form.fields, ...Object.keys(r.data).filter(key => !form.fields.some(f => f.key === key)).map(key => ({ key, label: `${key} (previous field)` }))].map((f) => (
                  <React.Fragment key={f.key}><dt className="text-muted-foreground">{f.label}</dt><dd className="whitespace-pre-wrap break-words">{r.data[f.key] ?? ""}</dd></React.Fragment>
                ))}
              </dl>
            </li>
          ))}
        </ul>
      )}
    </Modal>
  );
}
