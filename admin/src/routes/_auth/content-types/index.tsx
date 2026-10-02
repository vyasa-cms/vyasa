import * as React from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Boxes, Plus } from "lucide-react";
import { api, type ContentTypeInfo } from "@/api/client";
import { useCapabilities } from "@/lib/capabilities";
import { Button } from "@/components/ui/button";
import { DataList, type Column } from "@/components/ui/data-list";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, EmptyState, Field, PageHeader } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { Input } from "@/components/ui/input";
import { errorText } from "@/lib/error-text";
import { TYPE_SLUG_RE } from "@/lib/fields";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/content-types/")({
  component: ContentTypesPage,
});

const OWNER_CHIP: Record<ContentTypeInfo["owner"], { label: string; tone: "neutral" | "info" | "success" }> = {
  builtin: { label: "Built-in", tone: "neutral" },
  plugin: { label: "Plugin", tone: "info" },
  admin: { label: "Yours", tone: "success" },
};

function action(label: string, onClick: () => void, opts: { danger?: boolean } = {}) {
  return (
    <button type="button" onClick={onClick} className={cn("rounded-md px-2 py-1 text-xs font-medium hover:bg-accent", opts.danger ? "text-destructive hover:bg-destructive-subtle" : "text-muted-foreground hover:text-foreground")}>
      {label}
    </button>
  );
}

/**
 * What deleting a type needs: every entry of it gone, whatever its status.
 * The list's `count` is published entries only, which undercounts, so the
 * total is fetched here, while the dialog is already open. Someone who
 * sees only their own entries gets no number; the server's refusal still
 * names it.
 */
function DeleteTypeNote({ slug, counts }: { slug: string; counts: boolean }) {
  const total = useQuery({
    queryKey: ["posts", "type-total", slug],
    queryFn: async () => (await api.listPosts({ type: slug, per_page: 1 })).total,
    enabled: counts,
    staleTime: 0,
  });
  let text: string;
  if (!counts || total.isError) {
    text =
      "Its fields go with it. A type can only be deleted once it has no entries at all, drafts and trash included; the server says how many are left if any.";
  } else if (total.isPending) {
    text = "Counting its entries…";
  } else {
    const n = total.data;
    text =
      n > 0
        ? `${n} ${n === 1 ? "entry" : "entries"} (drafts and trash included) must be deleted first, or the server will refuse.`
        : "It has no entries; its fields go with it.";
  }
  return <span aria-live="polite">{text}</span>;
}

/**
 * Every kind of content the site holds. Built-in types and plugins' types
 * are listed for their fields only; an administrator's own types can also
 * be relabelled and deleted. The slug never changes: it is in addresses
 * and template names.
 */
function ContentTypesPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const caps = useCapabilities();
  const [creating, setCreating] = React.useState(false);
  const [editing, setEditing] = React.useState<ContentTypeInfo | null>(null);
  // A refused delete names how many entries are in the way; it stays on
  // the page until dismissed rather than leaving with a toast.
  const [refusal, setRefusal] = React.useState<string | null>(null);

  const types = useQuery({ queryKey: ["content-types"], queryFn: () => api.listContentTypes() });
  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["content-types"] });

  const remove = useMutation({
    mutationFn: (t: ContentTypeInfo) => api.deleteContentType(t.slug),
    onSuccess: (_r, t) => {
      setRefusal(null);
      invalidate();
      notify.success(`${t.plural} deleted`);
    },
    onError: (e, t) => setRefusal(`Couldn't delete ${t.plural}: ${errorText(e)}`),
  });

  // The dialog opens at once; the entry count fills in when it arrives
  // (see `DeleteTypeNote`), so a second click never queues a second one.
  const askDelete = async (t: ContentTypeInfo) => {
    const ok = await confirm({
      title: `Delete ${t.plural}?`,
      description: <DeleteTypeNote slug={t.slug} counts={caps.can("edit_others")} />,
      confirmLabel: "Delete type",
      destructive: true,
    });
    if (ok) remove.mutate(t);
  };

  const columns: Column<ContentTypeInfo>[] = [
    {
      key: "type",
      header: "Type",
      primary: true,
      render: (t) => (
        <span className="min-w-0">
          <span className="flex flex-wrap items-center gap-1.5">
            <span className="truncate font-medium">{t.plural}</span>
            <Chip tone={OWNER_CHIP[t.owner].tone} dot={false}>{OWNER_CHIP[t.owner].label}</Chip>
            {t.public ? null : <Chip tone="neutral" dot={false}>Not public</Chip>}
          </span>
          <span className="block truncate font-mono text-[11px] text-muted-foreground">/{t.slug}</span>
          {t.description !== "" ? <span className="block truncate text-xs text-muted-foreground">{t.description}</span> : null}
          {t.owner === "plugin" ? <span className="block text-[11px] text-muted-foreground">Comes from a plugin; it goes away when the plugin does. Its fields can still be added here.</span> : null}
        </span>
      ),
    },
    {
      key: "count",
      header: "Published",
      width: "7rem",
      render: (t) => <span className="tabular-nums text-muted-foreground">{t.count}</span>,
    },
  ];

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Content types"
        description="The kinds of content your site holds, and the fields each one has. Posts, pages and plugins' types are fixed; add your own for products, events, people…"
        actions={<Button size="sm" onClick={() => setCreating(true)}><Plus className="h-4 w-4" aria-hidden="true" />New content type</Button>}
      />

      {refusal !== null ? (
        <div role="alert" className="flex items-start gap-3 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm">
          <span className="min-w-0 flex-1">{refusal}</span>
          <button type="button" className="text-xs underline" onClick={() => setRefusal(null)}>Dismiss</button>
        </div>
      ) : null}

      <DataList
        testId="content-types"
        rows={types.data ?? []}
        columns={columns}
        rowKey={(t) => t.slug}
        isLoading={types.isPending}
        error={types.error}
        rowActions={(t) => (
          <span className="flex flex-wrap items-center gap-0.5">
            <Link
              to="/content-types/$slug"
              params={{ slug: t.slug }}
              className="rounded-md px-2 py-1 text-xs font-medium text-muted-foreground hover:bg-accent hover:text-foreground"
              aria-label={`Fields of ${t.plural}`}
            >
              Fields
            </Link>
            {t.owner === "admin" ? (
              <>
                {action("Edit labels", () => setEditing(t))}
                {action("Delete", () => void askDelete(t), { danger: true })}
              </>
            ) : null}
          </span>
        )}
        empty={<EmptyState icon={Boxes} title="No content types" description="Posts and pages should always be here; reload the page." />}
      />

      {creating || editing !== null ? (
        <TypeForm
          key={editing?.slug ?? "new"}
          editing={editing}
          onClose={() => { setCreating(false); setEditing(null); }}
          onSaved={invalidate}
        />
      ) : null}
    </div>
  );
}

/** Create a type, or relabel one of the administrator's own. */
function TypeForm({
  editing,
  onClose,
  onSaved,
}: {
  editing: ContentTypeInfo | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  // Mounted fresh for each opening (see the caller), so it starts from
  // what it edits before the first keystroke lands.
  const [v, setV] = React.useState(() =>
    editing === null
      ? { slug: "", singular: "", plural: "", description: "", public: true, has_archive: true }
      : { slug: editing.slug, singular: editing.singular, plural: editing.plural, description: editing.description, public: editing.public, has_archive: editing.has_archive },
  );

  const slug = v.slug.trim();
  const singular = v.singular.trim();
  const plural = v.plural.trim();
  const slugOk = editing !== null || TYPE_SLUG_RE.test(slug);
  const slugError =
    editing !== null || slug === ""
      ? null
      : !/^[a-z]/.test(slug)
        ? "Start with a letter."
        : slug.length > 32
          ? "At most 32 characters."
          : !TYPE_SLUG_RE.test(slug)
            ? "Use lowercase letters, digits and hyphens, 2 to 32 characters."
            : null;
  const valid = slugOk && singular !== "" && plural !== "";

  const save = useMutation({
    mutationFn: () => {
      const labels = { singular, plural, description: v.description.trim(), public: v.public, has_archive: v.has_archive };
      return editing === null ? api.createContentType({ slug, ...labels }) : api.updateContentType(editing.slug, labels);
    },
    onSuccess: () => {
      notify.success(
        editing === null ? `${plural} created` : `${plural} saved`,
        editing === null ? "Add its fields next, then find it in the navigation." : "",
      );
      onSaved();
      onClose();
    },
    onError: (e) => notify.error(editing === null ? "Couldn't create the content type" : "Couldn't save the content type", e),
  });

  return (
    <Modal
      open
      onClose={onClose}
      title={editing === null ? "New content type" : `Edit ${editing.plural}`}
      description={editing === null ? "A kind of content with its own list, editor and addresses." : "Labels and switches. The slug stays as it is."}
      footer={
        <>
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          <Button disabled={!valid || save.isPending} onClick={() => save.mutate()}>
            {save.isPending ? "Saving…" : editing === null ? "Create type" : "Save changes"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field
          label="Slug"
          htmlFor="type-slug"
          hint={
            editing === null
              ? "2 to 32 characters: lowercase letters, digits and hyphens, starting with a letter. It becomes the address (/slug/…) and template names, so it can't be changed later. Words the site already uses (post, page, admin, api, feed, tag, search…) and plugins' types are refused."
              : "The slug is in addresses and template names, so it can't be changed."
          }
          error={slugError}
        >
          <Input id="type-slug" autoFocus={editing === null} disabled={editing !== null} value={v.slug} onChange={(e) => setV({ ...v, slug: e.target.value })} placeholder="product" className="font-mono text-sm" />
        </Field>
        <div className="grid gap-4 sm:grid-cols-2">
          <Field label="Singular label" htmlFor="type-singular">
            <Input id="type-singular" autoFocus={editing !== null} value={v.singular} onChange={(e) => setV({ ...v, singular: e.target.value })} placeholder="Product" />
          </Field>
          <Field label="Plural label" htmlFor="type-plural">
            <Input id="type-plural" value={v.plural} onChange={(e) => setV({ ...v, plural: e.target.value })} placeholder="Products" />
          </Field>
        </div>
        <Field label="Description" htmlFor="type-description" hint="Optional. What the type is for.">
          <textarea id="type-description" rows={2} value={v.description} onChange={(e) => setV({ ...v, description: e.target.value })} className="w-full resize-y rounded-md border border-input bg-background p-2 text-sm" />
        </Field>
        <label className="flex items-start gap-2 text-sm">
          <input type="checkbox" checked={v.public} onChange={(e) => setV({ ...v, public: e.target.checked })} className="mt-0.5 h-3.5 w-3.5 accent-primary" />
          <span>Public — published entries get their own address on the site</span>
        </label>
        <label className="flex items-start gap-2 text-sm">
          <input type="checkbox" checked={v.has_archive} onChange={(e) => setV({ ...v, has_archive: e.target.checked })} className="mt-0.5 h-3.5 w-3.5 accent-primary" />
          <span>Archive — /{slug === "" ? "slug" : slug} lists the published entries</span>
        </label>
      </div>
    </Modal>
  );
}
