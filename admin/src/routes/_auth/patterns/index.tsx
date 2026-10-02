import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Bookmark } from "lucide-react";
import { api, type Pattern } from "@/api/client";
import { BlockEditor } from "@/components/editor/BlockEditor";
import type { Block } from "@/components/editor/blocks";
import { Button } from "@/components/ui/button";
import { DataList, type Column } from "@/components/ui/data-list";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Chip, EmptyState, Field, PageHeader } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";

export const Route = createFileRoute("/_auth/patterns/")({ component: PatternsPage });

/**
 * Saved arrangements of blocks. Authors make them from any block's handle
 * in the editor; this page renames, recategorises, edits and removes them.
 * Editing a synced pattern changes every page that uses it.
 */
export function PatternsPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const patterns = useQuery({ queryKey: ["patterns"], queryFn: () => api.listPatterns() });
  const [editing, setEditing] = React.useState<Pattern | null>(null);
  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["patterns"] });

  const remove = useMutation({
    mutationFn: (p: Pattern) => api.deletePattern(p.id),
    onSuccess: () => { invalidate(); notify.success("Pattern removed"); },
    onError: (e) => notify.error("Couldn't remove the pattern", e),
  });
  const askRemove = async (p: Pattern) => {
    const ok = await confirm({ title: `Remove "${p.name}"?`, description: p.synced ? "Pages that use this synced pattern will show nothing where it stood." : "Pages that copied it keep their copies.", confirmLabel: "Remove", destructive: true });
    if (ok) remove.mutate(p);
  };

  const rows = patterns.data ?? [];
  const columns: Column<Pattern>[] = [
    { key: "name", header: "Pattern", primary: true, render: (p) => <span className="min-w-0"><button type="button" className="block truncate font-medium underline-offset-2 hover:underline" onClick={() => setEditing(p)}>{p.name}</button><span className="block text-[11px] text-muted-foreground">{p.category || "uncategorised"} · {(p.blocks as Block[]).length} {(p.blocks as Block[]).length === 1 ? "block" : "blocks"}</span></span> },
    { key: "kind", header: "Kind", width: "9rem", render: (p) => <Chip tone={p.synced ? "info" : "neutral"} dot={false}>{p.synced ? "Synced" : "Copy on insert"}</Chip> },
    { key: "updated", header: "Updated", width: "10rem", render: (p) => <span className="text-xs text-muted-foreground">{new Date(p.updated_at).toLocaleDateString()}</span> },
  ];

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader title="Patterns" description="Blocks you use again and again. Save one from any block's handle in the editor; insert them with the / menu." />
      <DataList
        testId="patterns-table"
        rows={rows}
        columns={columns}
        rowKey={(p) => p.id}
        isLoading={patterns.isPending}
        error={patterns.error}
        rowActions={(p) => (
          <span className="flex gap-0.5">
            <Button size="sm" variant="ghost" className="h-7 text-xs" onClick={() => setEditing(p)}>Edit</Button>
            <Button size="sm" variant="ghost" className="h-7 text-xs text-destructive" onClick={() => void askRemove(p)}>Remove</Button>
          </span>
        )}
        empty={<EmptyState icon={Bookmark} title="No patterns yet" description="In the editor, hover a block, press the bookmark on its handle, and give it a name." />}
      />
      {editing ? <PatternForm pattern={editing} onClose={() => setEditing(null)} onSaved={invalidate} /> : null}
    </div>
  );
}

function PatternForm({ pattern, onClose, onSaved }: { pattern: Pattern; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = React.useState(pattern.name);
  const [category, setCategory] = React.useState(pattern.category);
  const [synced, setSynced] = React.useState(pattern.synced);
  const [blocks, setBlocks] = React.useState<Block[]>(pattern.blocks as Block[]);
  const save = useMutation({
    mutationFn: () => api.updatePattern(pattern.id, { name: name.trim(), category: category.trim(), synced, blocks }),
    onSuccess: () => { onSaved(); onClose(); notify.success("Pattern saved", synced ? "Every page that uses it shows the new version." : undefined); },
    onError: (e) => notify.error("Couldn't save the pattern", e),
  });
  return (
    <Modal open onClose={onClose} size="lg" title={`Edit "${pattern.name}"`} description={synced ? "Synced: pages follow this version." : "Copied on insert: pages keep what they copied."} testId="pattern-form"
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button disabled={name.trim() === "" || blocks.length === 0 || save.isPending} onClick={() => save.mutate()}>{save.isPending ? "Saving…" : "Save"}</Button></>}>
      <div className="space-y-3">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Name" htmlFor="pt-name"><Input id="pt-name" value={name} onChange={(e) => setName(e.target.value)} /></Field>
          <Field label="Category" htmlFor="pt-cat" hint="Groups the / menu."><Input id="pt-cat" value={category} onChange={(e) => setCategory(e.target.value)} placeholder="Marketing" /></Field>
        </div>
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" className="accent-primary" checked={synced} onChange={(e) => setSynced(e.target.checked)} />Synced: one source, every page follows</label>
        <div className="rounded-md border p-2"><BlockEditor value={blocks} onChange={setBlocks} /></div>
      </div>
    </Modal>
  );
}
