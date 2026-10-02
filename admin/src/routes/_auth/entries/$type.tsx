import { useCapabilities } from "@/lib/capabilities";
import { useMe } from "@/components/auth";
import * as React from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Boxes, ExternalLink, PlusCircle, RotateCcw, SquarePen, Trash2 } from "lucide-react";
import { api, type PostResponse } from "@/api/client";
import { Button, buttonVariants } from "@/components/ui/button";
import { DataList, FilterSelect, type Column } from "@/components/ui/data-list";
import { useConfirm } from "@/components/ui/dialog";
import { EmptyState, ErrorNote, PageHeader, StatusChip } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { formatRelative } from "@/routes/_auth/index";
import { cn } from "@/lib/utils";

/**
 * The entries of one plugin's or administrator's content type. Writing one
 * happens in the usual editor (`/posts/new?type=<slug>`), with the type's
 * fields in its Fields panel.
 */
export const Route = createFileRoute("/_auth/entries/$type")({
  component: function Entries() {
    const { type } = Route.useParams();
    // Keyed so moving between two types starts each list afresh.
    return <EntriesPage key={type} slug={type} />;
  },
});

const PER_PAGE = 20;

const STATUS_OPTIONS = [
  { value: "", label: "Any status" },
  { value: "published", label: "Published" },
  { value: "draft", label: "Draft" },
  { value: "scheduled", label: "Scheduled" },
  { value: "private", label: "Private" },
  { value: "trash", label: "Trash" },
];

function EntriesPage({ slug }: { slug: string }) {
  const caps = useCapabilities();
  const me = useMe();
  const author = caps.can("edit_others") ? undefined : me.data?.id;
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [page, setPage] = React.useState(1);
  const [term, setTerm] = React.useState("");
  const [debounced, setDebounced] = React.useState("");
  const [status, setStatus] = React.useState("");
  const [selected, setSelected] = React.useState<Set<string | number>>(new Set());

  React.useEffect(() => {
    const t = setTimeout(() => setDebounced(term), 300);
    return () => clearTimeout(t);
  }, [term]);

  const types = useQuery({ queryKey: ["content-types"], queryFn: () => api.listContentTypes(), staleTime: 60_000 });
  const type = types.data?.find((t) => t.slug === slug);
  const plural = type?.plural ?? slug;
  const singular = type?.singular ?? "entry";

  const entries = useQuery({
    queryKey: ["posts", "entries", slug, page, debounced, status, author],
    enabled: caps.isSuccess && !!me.data,
    queryFn: () =>
      api.listPosts({
        type: slug,
        author_id: author,
        page,
        per_page: PER_PAGE,
        ...(status === "" ? {} : { status }),
        ...(debounced === "" ? {} : { search: debounced }),
      }),
    placeholderData: keepPreviousData,
  });

  const trash = useMutation({
    mutationFn: (id: string) => api.trashPost(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["posts"] });
      notify.success("Moved to trash");
    },
    onError: (e) => notify.error("Couldn't move to trash", e),
  });
  const restore = useMutation({
    mutationFn: (id: string) => api.restorePost(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["posts"] });
      notify.success("Restored");
    },
    onError: (e) => notify.error("Couldn't restore", e),
  });
  const bulk = useMutation({
    mutationFn: ({ ids, action }: { ids: string[]; action: "restore" | "trash"; label: string }) => api.batchPosts(ids, action),
    onSuccess: (r, { label }) => {
      setSelected(new Set());
      void queryClient.invalidateQueries({ queryKey: ["posts"] });
      if (r.failed.length > 0) notify.error(`${label}: ${r.done} done, ${r.failed.length} skipped`, r.failed[0]?.message ?? "");
      else notify.success(`${label}: ${r.done}`);
    },
    onError: (e) => notify.error("Couldn't apply that", e),
  });
  const askTrash = async (row: PostResponse) => {
    const ok = await confirm({
      title: `Move “${row.title || "(untitled)"}” to trash?`,
      description: "It stops appearing on your site. You can restore it later.",
      confirmLabel: "Move to trash",
      destructive: true,
    });
    if (ok) trash.mutate(row.id);
  };

  const items = entries.data?.items ?? [];
  const total = entries.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PER_PAGE));

  if (types.isSuccess && type === undefined) {
    return <ErrorNote title="No such content type" error={`There is no content type “${slug}”. It may have been deleted, or its plugin turned off.`} />;
  }

  const newLink = (label: string) => (
    <Link to="/posts/$postId" params={{ postId: "new" }} search={{ type: slug }} className={cn(buttonVariants({ size: "sm" }))}>
      <PlusCircle className="h-4 w-4" aria-hidden="true" />
      {label}
    </Link>
  );

  const columns: Column<PostResponse>[] = [
    {
      key: "title",
      header: "Title",
      primary: true,
      render: (p) => (
        <Link to="/posts/$postId" params={{ postId: String(p.id) }} className="block min-w-0">
          <span className="block truncate font-medium hover:underline">
            {p.title || <span className="text-muted-foreground">(untitled)</span>}
          </span>
          <span className="block truncate font-mono text-[11px] text-muted-foreground">{p.public_url ?? `/${slug}/${p.slug}`}</span>
        </Link>
      ),
    },
    { key: "status", header: "Status", width: "7rem", render: (p) => <StatusChip status={p.status} /> },
    {
      key: "updated",
      header: "Updated",
      width: "8rem",
      hideBelow: "md",
      render: (p) => <span className="truncate text-muted-foreground">{formatRelative(p.updated_at)}</span>,
    },
  ];

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title={plural}
        description={entries.isPending ? "Loading…" : `${total} ${total === 1 ? singular.toLowerCase() : plural.toLowerCase()}`}
        actions={newLink(`New ${singular.toLowerCase()}`)}
      />
      <DataList
        testId="entries-table"
        rowTestId="entry-row"
        rows={items}
        columns={columns}
        rowKey={(p) => p.id}
        isLoading={entries.isPending}
        error={entries.error}
        search={{ value: term, onChange: (value) => { setTerm(value); setPage(1); }, placeholder: `Search ${plural.toLowerCase()}` }}
        filters={<FilterSelect label="Status" value={status} options={STATUS_OPTIONS} onChange={(v) => { setStatus(v); setPage(1); setSelected(new Set()); }} />}
        selection={{
          selected,
          onChange: setSelected,
          // As on the posts list: the trash view restores, the others trash.
          bulkActions: (sel) => {
            const ids = [...sel].map(String);
            const [action, label] = status === "trash" ? (["restore", "Restore"] as const) : (["trash", "Move to trash"] as const);
            // Restoring needs no warning; trashing many asks first, as on
            // the posts list.
            const run = async () => {
              if (action === "trash") {
                const one = ids.length === 1 ? items.find((p) => String(p.id) === ids[0]) : undefined;
                const ok = await confirm({
                  title:
                    one !== undefined
                      ? `Move “${one.title || "(untitled)"}” to trash?`
                      : `Move ${ids.length} ${(ids.length === 1 ? singular : plural).toLowerCase()} to trash?`,
                  description: "Trashed entries stop appearing on your site. You can restore them later.",
                  confirmLabel: "Move to trash",
                  destructive: true,
                });
                if (!ok) return;
              }
              bulk.mutate({ ids, action, label });
            };
            return (
              <Button size="sm" variant="outline" className="h-7" disabled={bulk.isPending} onClick={() => void run()}>{label}</Button>
            );
          },
        }}
        rowActions={(p) => (
          <>
            <Link
              to="/posts/$postId"
              params={{ postId: String(p.id) }}
              aria-label={`Edit ${p.title || "untitled"}`}
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <SquarePen className="h-4 w-4" aria-hidden="true" />
            </Link>
            {p.public_url ? (
              <a
                href={p.public_url}
                target="_blank"
                rel="noreferrer"
                aria-label={`View ${p.title || "untitled"} on the site`}
                className="hidden h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground sm:inline-flex"
              >
                <ExternalLink className="h-4 w-4" aria-hidden="true" />
              </a>
            ) : null}
            {p.status === "trash" ? (
              <Button
                size="sm"
                variant="outline"
                className="h-8"
                disabled={restore.isPending}
                aria-label={`Restore ${p.title || "untitled"}`}
                onClick={() => restore.mutate(p.id)}
              >
                <RotateCcw className="h-4 w-4" aria-hidden="true" />
                Restore
              </Button>
            ) : (
              <button
                type="button"
                onClick={() => void askTrash(p)}
                aria-label={`Move ${p.title || "untitled"} to trash`}
                className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-destructive-subtle hover:text-destructive"
              >
                <Trash2 className="h-4 w-4" aria-hidden="true" />
              </button>
            )}
          </>
        )}
        empty={
          <EmptyState
            icon={Boxes}
            title={debounced === "" && status === "" ? `No ${plural.toLowerCase()} yet` : `No ${plural.toLowerCase()} match`}
            description={debounced === "" && status === "" ? `Start the first ${singular.toLowerCase()}.` : "Try a different search or status."}
          />
        }
        pagination={{ page, totalPages, total, onPage: setPage, label: `${total} · page ${page} of ${totalPages}` }}
      />
    </div>
  );
}
