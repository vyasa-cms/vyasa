import { useCapabilities } from "@/lib/capabilities";
import { useMe } from "@/components/auth";
import * as React from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ExternalLink, Files, PlusCircle, SquarePen, Trash2 } from "lucide-react";
import { api, type PostResponse } from "@/api/client";
import { Button, buttonVariants } from "@/components/ui/button";
import { DataList, type Column } from "@/components/ui/data-list";
import { useConfirm } from "@/components/ui/dialog";
import { EmptyState, PageHeader, StatusChip } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { formatRelative } from "@/routes/_auth/index";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/pages/")({
  component: PagesPage,
});

const PER_PAGE = 20;

function PagesPage() {
  const caps = useCapabilities();
  const me = useMe();
  const author = caps.can("edit_others") ? undefined : me.data?.id;
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [page, setPage] = React.useState(1);
  const [term, setTerm] = React.useState("");
  const [debounced, setDebounced] = React.useState("");
  const [selected, setSelected] = React.useState<Set<string | number>>(new Set());

  React.useEffect(() => {
    const t = setTimeout(() => setDebounced(term), 300);
    return () => clearTimeout(t);
  }, [term]);

  const pages = useQuery({
    queryKey: ["posts", "pages", page, debounced, author],
    enabled: caps.isSuccess && !!me.data,
    queryFn: () =>
      api.listPosts({
        type: "page",
        author_id: author,
        page,
        per_page: PER_PAGE,
        ...(debounced === "" ? {} : { search: debounced }),
      }),
    placeholderData: keepPreviousData,
  });

  const trash = useMutation({
    mutationFn: (ids: string[]) => Promise.all(ids.map((id) => api.trashPost(id))),
    onSuccess: (_r, ids) => {
      setSelected(new Set());
      void queryClient.invalidateQueries({ queryKey: ["posts"] });
      notify.success(
        ids.length === 1 ? "Page moved to trash" : `${ids.length} pages moved to trash`,
      );
    },
    onError: (e) => notify.error("Couldn't move to trash", e),
  });

  const items = pages.data?.items ?? [];
  const total = pages.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PER_PAGE));
  React.useEffect(() => { if (pages.isSuccess && !pages.isPlaceholderData && page > totalPages) { setSelected(new Set()); setPage(totalPages); } }, [pages.isSuccess, pages.isPlaceholderData, page, totalPages]);
  const byId = new Map(items.map((p) => [p.id, p]));

  const askTrash = async (rows: PostResponse[]) => {
    const one = rows.length === 1 ? rows[0] : undefined;
    const ok = await confirm({
      title:
        one !== undefined
          ? `Move “${one.title || "(untitled)"}” to trash?`
          : `Move ${rows.length} pages to trash?`,
      description:
        "The page stops being reachable on your site. You can restore it later.",
      confirmLabel: "Move to trash",
      destructive: true,
    });
    if (ok) trash.mutate(rows.map((r) => r.id));
  };

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
          <span className="block truncate font-mono text-[11px] text-muted-foreground">
            /{p.slug}
          </span>
        </Link>
      ),
    },
    {
      key: "status",
      header: "Status",
      width: "7rem",
      render: (p) => <StatusChip status={p.status} />,
    },
    {
      key: "updated",
      header: "Updated",
      width: "8rem",
      hideBelow: "md",
      render: (p) => (
        <span className="truncate text-muted-foreground">
          {formatRelative(p.updated_at)}
        </span>
      ),
    },
  ];

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Pages"
        description="Standalone content like About or Contact, published at its own address."
        actions={
          <Link to="/posts/$postId" params={{ postId: "new" }} search={{ type: "page" }} className={cn(buttonVariants({ size: "sm" }))}>
            <PlusCircle className="h-4 w-4" aria-hidden="true" />
            New page
          </Link>
        }
      />

      <DataList
        rows={items}
        columns={columns}
        rowKey={(p) => p.id}
        isLoading={pages.isPending}
        error={pages.error}
        search={{ value: term, onChange: (value) => { setTerm(value); setPage(1); setSelected(new Set()); }, placeholder: "Search pages" }}
        selection={{
          selected,
          onChange: setSelected,
          bulkActions: (sel) => (
            <Button
              size="sm"
              variant="outline"
              className="h-7"
              onClick={() =>
                void askTrash(
                  [...sel]
                    .map((id) => byId.get(String(id)))
                    .filter((p): p is PostResponse => p !== undefined),
                )
              }
            >
              <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
              Trash
            </Button>
          ),
        }}
        rowActions={(p) => (
          <>
            <Link
              to="/posts/$postId"
              params={{ postId: String(p.id) }}
              aria-label={`Edit ${p.title || "untitled page"}`}
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <SquarePen className="h-4 w-4" aria-hidden="true" />
            </Link>
            <a
              href={`/${p.slug}`}
              target="_blank"
              rel="noreferrer"
              aria-label={`View ${p.title || "untitled page"}`}
              className="hidden h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground sm:inline-flex"
            >
              <ExternalLink className="h-4 w-4" aria-hidden="true" />
            </a>
            <button
              type="button"
              onClick={() => void askTrash([p])}
              aria-label={`Move ${p.title || "untitled page"} to trash`}
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-destructive-subtle hover:text-destructive"
            >
              <Trash2 className="h-4 w-4" aria-hidden="true" />
            </button>
          </>
        )}
        empty={
          <EmptyState
            icon={Files}
            title="No pages yet"
            description="Pages hold content that isn't part of your posting timeline — an about page, say."
            action={
              <Link to="/posts/$postId" params={{ postId: "new" }} search={{ type: "page" }} className={cn(buttonVariants({ size: "sm" }))}>
                Create a page
              </Link>
            }
          />
        }
        pagination={{
          page,
          totalPages,
          total,
          onPage: (n) => {
            setSelected(new Set());
            setPage(n);
          },
          label: `${total} pages · page ${page} of ${totalPages}`,
        }}
      />
    </div>
  );
}
