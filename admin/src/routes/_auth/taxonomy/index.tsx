import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { GitMerge, Plus, Tags, Trash2, SquarePen } from "lucide-react";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { DataList, FilterSelect, type Column } from "@/components/ui/data-list";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { EmptyState, Field, PageHeader } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { Input } from "@/components/ui/input";

export const Route = createFileRoute("/_auth/taxonomy/")({
  component: TaxonomyPage,
});

interface Term {
  id: string;
  name: string;
  slug: string;
  parent_id?: string | null;
  post_count?: number | null;
}

type Taxonomy = "category" | "tag";

export function TaxonomyPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [tax, setTax] = React.useState<Taxonomy>("category");
  const [term, setTerm] = React.useState("");
  const [creating, setCreating] = React.useState(false);
  const [editing, setEditing] = React.useState<Term | null>(null);
  const [merging, setMerging] = React.useState<Term | null>(null);

  const terms = useQuery({
    queryKey: ["terms", tax],
    queryFn: () => api.listTerms({ taxonomy: tax, post_counts: true }),
  });

  const invalidate = () =>
    void queryClient.invalidateQueries({ queryKey: ["terms"] });

  const remove = useMutation({
    mutationFn: (id: string) => api.deleteTerm(id),
    onSuccess: () => {
      invalidate();
      notify.success(`${label(tax)} deleted`);
    },
    onError: (e) => notify.error("Couldn't delete", e),
  });

  const merge = useMutation({
    mutationFn: ({ from, into }: { from: string; into: string }) =>
      api.mergeTerms(from, into),
    onSuccess: () => {
      setMerging(null);
      invalidate();
      notify.success("Terms merged", "Posts and children were reassigned.");
    },
    onError: (e) => notify.error("Couldn't merge", e),
  });

  const all = (terms.data ?? []) as Term[];
  const byId = new Map(all.map((t) => [t.id, t]));
  const filtered =
    term === ""
      ? all
      : all.filter(
          (t) =>
            t.name.toLowerCase().includes(term.toLowerCase()) ||
            t.slug.toLowerCase().includes(term.toLowerCase()),
        );

  const askDelete = async (t: Term) => {
    const used = t.post_count ?? 0;
    const ok = await confirm({
      title: `Delete “${t.name}”?`,
      description:
        used > 0
          ? `${used} ${used === 1 ? "post uses" : "posts use"} this ${label(tax).toLowerCase()}. They stay published, but lose this label.`
          : `This ${label(tax).toLowerCase()} isn't used by any posts.`,
      confirmLabel: "Delete",
      destructive: true,
    });
    if (ok) remove.mutate(t.id);
  };

  const columns: Column<Term>[] = [
    {
      key: "name",
      header: "Name",
      primary: true,
      render: (t) => (
        <span className="min-w-0">
          <span className="block truncate font-medium">
            {t.parent_id != null ? (
              <span className="text-muted-foreground">
                {byId.get(t.parent_id)?.name ?? "—"} ›{" "}
              </span>
            ) : null}
            {t.name}
          </span>
          <span className="block truncate font-mono text-[11px] text-muted-foreground">
            /{t.slug}
          </span>
        </span>
      ),
    },
    {
      key: "count",
      header: "Posts",
      width: "5rem",
      render: (t) => (
        <span className="tabular-nums text-muted-foreground">
          {t.post_count ?? 0}
        </span>
      ),
    },
  ];

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Categories & tags"
        description="Group your posts so readers can browse by subject."
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <Plus className="h-4 w-4" aria-hidden="true" />
            New {label(tax).toLowerCase()}
          </Button>
        }
      />

      <DataList
        rows={filtered}
        columns={columns}
        rowKey={(t) => t.id}
        isLoading={terms.isPending}
        error={terms.error}
        search={{ value: term, onChange: setTerm, placeholder: "Search" }}
        filters={
          <FilterSelect
            label="Type"
            value={tax}
            options={[
              { value: "category", label: "Categories" },
              { value: "tag", label: "Tags" },
            ]}
            onChange={(v) => setTax(v as Taxonomy)}
          />
        }
        rowActions={(t) => (
          <>
            <button type="button" onClick={() => setEditing(t)} aria-label={`Edit ${t.name}`} className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent">
              <SquarePen className="h-4 w-4" aria-hidden="true" />
            </button>
            <button
              type="button"
              onClick={() => setMerging(t)}
              aria-label={`Merge ${t.name} into another ${label(tax).toLowerCase()}`}
              title="Merge into another"
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <GitMerge className="h-4 w-4" aria-hidden="true" />
            </button>
            <button
              type="button"
              onClick={() => void askDelete(t)}
              aria-label={`Delete ${t.name}`}
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-destructive-subtle hover:text-destructive"
            >
              <Trash2 className="h-4 w-4" aria-hidden="true" />
            </button>
          </>
        )}
        empty={
          <EmptyState
            icon={Tags}
            title={term === "" ? `No ${label(tax).toLowerCase()}s yet` : "Nothing matches"}
            description={
              term === ""
                ? `Create one to start grouping posts.`
                : "Try a different search."
            }
            action={
              term === "" ? (
                <Button size="sm" variant="outline" onClick={() => setCreating(true)}>
                  Create the first one
                </Button>
              ) : null
            }
          />
        }
      />

      <CreateTermModal
        open={creating || editing !== null}
        editing={editing}
        onClose={() => { setCreating(false); setEditing(null); }}
        taxonomy={tax}
        parents={all}
        onCreated={invalidate}
      />

      <MergeModal
        term={merging}
        candidates={all}
        onClose={() => setMerging(null)}
        onMerge={(into) => {
          if (merging !== null) merge.mutate({ from: merging.id, into });
        }}
        pending={merge.isPending}
      />
    </div>
  );
}

function label(tax: Taxonomy): string {
  return tax === "category" ? "Category" : "Tag";
}

function CreateTermModal({
  editing,
  open,
  onClose,
  taxonomy,
  parents,
  onCreated,
}: {
  editing: Term | null;
  open: boolean;
  onClose: () => void;
  taxonomy: Taxonomy;
  parents: Term[];
  onCreated: () => void;
}) {
  const [name, setName] = React.useState("");
  const [slug, setSlug] = React.useState("");
  const [parentId, setParentId] = React.useState("");

  React.useEffect(() => {
    if (open) { setName(editing?.name ?? ""); setSlug(editing?.slug ?? ""); setParentId(editing?.parent_id ?? ""); }
  }, [open, editing]);
  const allowedParents = parents.filter((candidate) => {
    const visited = new Set<string>();
    let current: Term | undefined = candidate;
    while (current) {
      if (current.id === editing?.id || visited.has(current.id)) return false;
      visited.add(current.id);
      current = parents.find((p) => p.id === current?.parent_id);
    }
    return true;
  });
  const create = useMutation({
    mutationFn: () => editing ? api.updateTerm(editing.id, {
      name, slug: slug.trim() || undefined, parent_id: parentId || null,
    }) : api.createTerm({
        taxonomy,
        name,
        slug: slug === "" ? null : slug,
        parent_id: parentId === "" ? null : parentId,
      }),
    onSuccess: () => {
      notify.success(`${label(taxonomy)} “${name}” ${editing ? "updated" : "created"}`);
      setName("");
      setSlug("");
      setParentId("");
      onCreated();
      onClose();
    },
    onError: (e) => notify.error("Couldn't save", e),
  });

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={`${editing ? "Edit" : "New"} ${label(taxonomy).toLowerCase()}`}
      description="Names appear on your site; the address is derived from the name unless you set one."
      footer={
        <>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={name.trim() === "" || create.isPending}
            onClick={() => create.mutate()}
          >
            {create.isPending ? "Saving…" : editing ? "Save changes" : "Create"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label="Name" htmlFor="term-name">
          <Input
            id="term-name"
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Engineering"
          />
        </Field>
        <Field
          label="Address"
          htmlFor="term-slug"
          hint="Leave blank to generate it from the name."
        >
          <Input
            id="term-slug"
            value={slug}
            onChange={(e) => setSlug(e.target.value)}
            placeholder="engineering"
          />
        </Field>
        {taxonomy === "category" ? (
          <Field
            label="Parent category"
            htmlFor="term-parent"
            hint="Optional — nest this under an existing category."
          >
            <select
              id="term-parent"
              value={parentId}
              onChange={(e) => setParentId(e.target.value)}
              className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            >
              <option value="">No parent</option>
              {allowedParents.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </Field>
        ) : null}
      </div>
    </Modal>
  );
}

/**
 * Merge by picking a target from a list, not by reading two integers off a
 * table and retyping them.
 */
function MergeModal({
  term,
  candidates,
  onClose,
  onMerge,
  pending,
}: {
  term: Term | null;
  candidates: Term[];
  onClose: () => void;
  onMerge: (into: string) => void;
  pending: boolean;
}) {
  const [into, setInto] = React.useState("");
  const [filter, setFilter] = React.useState("");

  React.useEffect(() => {
    setInto("");
    setFilter("");
  }, [term]);

  const options = candidates.filter(
    (c) =>
      c.id !== term?.id &&
      (filter === "" || c.name.toLowerCase().includes(filter.toLowerCase())),
  );
  const target = candidates.find((c) => String(c.id) === into);
  const affected = term?.post_count ?? 0;

  return (
    <Modal
      open={term !== null}
      onClose={onClose}
      title={`Merge “${term?.name ?? ""}” into another`}
      description="Posts and any child terms move across, then this one is removed."
      footer={
        <>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={into === "" || pending}
            onClick={() => onMerge(into)}
          >
            {pending ? "Merging…" : "Merge"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label="Merge into" htmlFor="merge-filter">
          <Input
            id="merge-filter"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Filter by name"
          />
        </Field>

        <div className="max-h-56 overflow-y-auto rounded-md border">
          {options.length === 0 ? (
            <p className="px-3 py-4 text-sm text-muted-foreground">
              Nothing else to merge into.
            </p>
          ) : (
            <ul className="divide-y">
              {options.map((c) => (
                <li key={c.id}>
                  <label className="flex cursor-pointer items-center gap-3 px-3 py-2 text-sm hover:bg-accent">
                    <input
                      type="radio"
                      name="merge-target"
                      value={c.id}
                      checked={into === String(c.id)}
                      onChange={(e) => setInto(e.target.value)}
                      className="accent-primary"
                    />
                    <span className="min-w-0 flex-1 truncate">{c.name}</span>
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {c.post_count ?? 0} posts
                    </span>
                  </label>
                </li>
              ))}
            </ul>
          )}
        </div>

        {target !== undefined ? (
          <p className="rounded-md border border-warning/40 bg-warning-subtle px-3 py-2 text-sm text-warning">
            {affected} {affected === 1 ? "post moves" : "posts move"} to “
            {target.name}”, and “{term?.name}” is deleted.
          </p>
        ) : null}
      </div>
    </Modal>
  );
}
