import { useMe } from "@/components/auth";
import { useCapabilities } from "@/lib/capabilities";
import * as React from "react";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, ExternalLink, FileText, Pin, PlusCircle, SquarePen, Trash2 } from "lucide-react";
import { api, type ListPostsQuery, type PostResponse } from "@/api/client";
import { buttonVariants, Button } from "@/components/ui/button";
import { DataList, FilterSelect, type Column } from "@/components/ui/data-list";
import { useConfirm } from "@/components/ui/dialog";
import { EmptyState, PageHeader, StatusChip } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { formatRelative } from "@/routes/_auth/index";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/posts/")({
  component: PostsPage,
  validateSearch: (search: Record<string, unknown>) => ({
    page: search.page,
  }),
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

function PostsPage() {
  const caps = useCapabilities();
  const { can } = caps;
  const me = useMe();
  const search = Route.useSearch();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const confirm = useConfirm();

  const page = Math.max(1, Number(search.page ?? 1));
  const [term, setTerm] = React.useState("");
  const [status, setStatus] = React.useState("");
  const [selected, setSelected] = React.useState<Set<string | number>>(new Set());

  // Debounce so typing doesn't fire a request per keystroke.
  const [debounced, setDebounced] = React.useState("");
  React.useEffect(() => {
    const t = setTimeout(() => setDebounced(term), 300);
    return () => clearTimeout(t);
  }, [term]);

  const query: ListPostsQuery = {
    type: "post",
    ...(!can("edit_others") ? { author_id: me.data?.id } : {}),
    page,
    per_page: PER_PAGE,
    ...(status === "" ? {} : { status }),
    ...(debounced === "" ? {} : { search: debounced }),
  };

  const posts = useQuery({
    queryKey: ["posts", "list", query],
    enabled: caps.isSuccess && !!me.data,
    queryFn: () => api.listPosts(query),
    placeholderData: keepPreviousData,
  });

  const invalidate = () =>
    void queryClient.invalidateQueries({ queryKey: ["posts"] });

  const aiAvailable = useQuery({ queryKey: ["ai-available"], queryFn: () => api.aiAvailable() });
  const processAi = useMutation({
    mutationFn: async ({ id, feature }: { id: string; feature: "embeddings" | "autofill" }) => {
      if (feature === "embeddings") {
        const result = await api.embedPost(id);
        return result.embedded ? "Search embedding updated" : result.reason ?? "No change needed";
      }
      const result = await api.autofillPost(id);
      return result.filled.length ? `Filled ${result.filled.join(", ")}` : result.reason ?? "No empty fields to fill";
    },
    onSuccess: (message) => { invalidate(); notify.success(message); },
    onError: (e) => notify.error("Couldn't process the post", e),
  });
  const trash = useMutation({
    mutationFn: (ids: string[]) =>
      Promise.all(ids.map((id) => api.trashPost(id))),
    onSuccess: (_r, ids) => {
      setSelected(new Set());
      invalidate();
      notify.success(
        ids.length === 1 ? "Post moved to trash" : `${ids.length} posts moved to trash`,
      );
    },
    onError: (e) => notify.error("Couldn't move to trash", e),
  });

  const setPage = (next: number) => {
    setSelected(new Set());
    void navigate({ to: "/posts", search: { page: next } });
  };

  const bulk = useMutation({
    mutationFn: ({ ids, action }: { ids: string[]; action: Parameters<typeof api.batchPosts>[1]; label: string }) => api.batchPosts(ids, action),
    onSuccess: (r, { label }) => {
      setSelected(new Set());
      void queryClient.invalidateQueries({ queryKey: ["posts"] });
      if (r.failed.length > 0) notify.error(`${label}: ${r.done} done, ${r.failed.length} skipped`, r.failed[0]?.message ?? "");
      else notify.success(`${label}: ${r.done} ${r.done === 1 ? "post" : "posts"}`);
    },
    onError: (e) => notify.error("Couldn't apply that", e),
  });
  const duplicate = useMutation({
    mutationFn: (p: PostResponse) => api.duplicatePost(p.id),
    onSuccess: (copy) => {
      void queryClient.invalidateQueries({ queryKey: ["posts"] });
      notify.success("Copied as a draft", copy.title);
      void navigate({ to: "/posts/$postId", params: { postId: String(copy.id) } });
    },
    onError: (e) => notify.error("Couldn't duplicate", e),
  });
  const askTrash = async (rows: PostResponse[]) => {
    const one = rows.length === 1 ? rows[0] : undefined;
    const ok = await confirm({
      title:
        one !== undefined
          ? `Move “${one.title || "(untitled)"}” to trash?`
          : `Move ${rows.length} posts to trash?`,
      description:
        "Trashed posts stop appearing on your site. You can restore them later.",
      confirmLabel: "Move to trash",
      destructive: true,
    });
    if (ok) trash.mutate(rows.map((r) => r.id));
  };

  const total = posts.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PER_PAGE));
  React.useEffect(() => { if (posts.isSuccess && !posts.isPlaceholderData && page > totalPages) { setSelected(new Set()); void navigate({ to: "/posts", search: { page: totalPages }, replace: true }); } }, [posts.isSuccess, posts.isPlaceholderData, page, totalPages, navigate]);
  const items = posts.data?.items ?? [];
  const byId = new Map(items.map((p) => [p.id, p]));

  const columns: Column<PostResponse>[] = [
    {
      key: "title",
      header: "Title",
      primary: true,
      render: (post) => (
        <Link
          to="/posts/$postId"
          params={{ postId: String(post.id) }}
          className="block min-w-0"
        >
          <span className="block truncate font-medium hover:underline">
            {(post as unknown as { sticky?: boolean }).sticky ? <Pin className="mr-1 inline h-3 w-3 text-primary" aria-label="Pinned" /> : null}
            {post.title || (
              <span className="text-muted-foreground">(untitled)</span>
            )}
          </span>
          <span className="block truncate font-mono text-[11px] text-muted-foreground">
            {post.public_url ?? `/post/${post.slug}`}
          </span>
        </Link>
      ),
    },
    {
      key: "status",
      header: "Status",
      width: "7rem",
      render: (post) => <StatusChip status={post.status} />,
    },
    {
      key: "updated",
      header: "Updated",
      width: "8rem",
      hideBelow: "md",
      render: (post) => (
        <span className="truncate text-muted-foreground">
          {formatRelative(post.updated_at)}
        </span>
      ),
    },
  ];

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Posts"
        description={
          posts.isPending ? "Loading…" : `${total} ${total === 1 ? "post" : "posts"}`
        }
        actions={
          <Link
            to="/posts/$postId" params={{ postId: "new" }}
            className={cn(buttonVariants({ size: "sm" }))}
            data-testid="new-post"
          >
            <PlusCircle className="h-4 w-4" aria-hidden="true" />
            New post
          </Link>
        }
      />

      <DataList
        testId="posts-table"
        rowTestId="post-row"
        totalTestId="posts-total"
        rows={items}
        columns={columns}
        rowKey={(p) => p.id}
        isLoading={posts.isPending}
        error={posts.error}
        search={{
          value: term,
          onChange: (value) => { setTerm(value); setPage(1); },
          placeholder: "Search posts",
        }}
        filters={
          <FilterSelect
            label="Status"
            value={status}
            options={STATUS_OPTIONS}
            onChange={(v) => {
              setStatus(v);
              setPage(1);
              setSelected(new Set());
            }}
          />
        }
        selection={{
          selected,
          onChange: setSelected,
          bulkActions: (sel) => {
            const ids = [...sel].map(String);
            const act = (action: Parameters<typeof api.batchPosts>[1], label: string) => (
              <Button size="sm" variant="outline" className="h-7" onClick={() => void bulk.mutateAsync({ ids, action, label })}>{label}</Button>
            );
            return (
              <>
                {status === "trash" ? act("restore", "Restore") : (
                  <>
                    {can("publish_posts") ? act("publish", "Publish") : null}
                    {act("draft", "Unpublish")}
                    {act("pin", "Pin")}
                    {act("unpin", "Unpin")}
                    <Button
                      size="sm"
                      variant="outline"
                      className="h-7"
                      onClick={() =>
                        void askTrash(
                          ids.map((id) => byId.get(id)).filter((p): p is PostResponse => p !== undefined),
                        )
                      }
                    >
                      <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                      Trash
                    </Button>
                  </>
                )}
              </>
            );
          },
        }}
        rowActions={(post) => (
          <>
            {aiAvailable.data?.autofill === true ? <Button size="sm" variant="outline" disabled={processAi.isPending} onClick={() => processAi.mutate({ id: post.id, feature: "autofill" })}>Fill empty fields</Button> : null}
            {aiAvailable.data?.embeddings === true && post.status === "published" ? <Button size="sm" variant="outline" disabled={processAi.isPending} onClick={() => processAi.mutate({ id: post.id, feature: "embeddings" })}>Update embedding</Button> : null}
            <Link
              to="/posts/$postId"
              params={{ postId: String(post.id) }}
              aria-label={`Edit ${post.title || "untitled post"}`}
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            >
              <SquarePen className="h-4 w-4" aria-hidden="true" />
            </Link>
            <a
              href={post.public_url ?? `/post/${post.slug}`}
              target="_blank"
              rel="noreferrer"
              aria-label={`View ${post.title || "untitled post"} on the site`}
              className="hidden h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground sm:inline-flex"
            >
              <ExternalLink className="h-4 w-4" aria-hidden="true" />
            </a>
            <button
              type="button"
              onClick={() => duplicate.mutate(post)}
              aria-label={`Duplicate ${post.title || "untitled post"}`}
              title="Duplicate"
              className="hidden h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground sm:inline-flex"
            >
              <Copy className="h-4 w-4" aria-hidden="true" />
            </button>
            <button
              type="button"
              onClick={() => void askTrash([post])}
              aria-label={`Move ${post.title || "untitled post"} to trash`}
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-destructive-subtle hover:text-destructive"
            >
              <Trash2 className="h-4 w-4" aria-hidden="true" />
            </button>
          </>
        )}
        empty={
          <EmptyState
            testId="posts-empty"
            icon={FileText}
            title={
              debounced === "" && status === ""
                ? "No posts yet"
                : "No posts match those filters"
            }
            description={
              debounced === "" && status === ""
                ? "Your first post is a good place to start."
                : "Try a different search term or clear the status filter."
            }
            action={
              debounced === "" && status === "" ? (
                <Link
                  to="/posts/$postId" params={{ postId: "new" }}
                  className={cn(buttonVariants({ size: "sm" }))}
                >
                  Write your first post
                </Link>
              ) : null
            }
          />
        }
        pagination={{
          page,
          totalPages,
          total,
          onPage: setPage,
          label: `${total} posts · page ${page} of ${totalPages}`,
        }}
      />
    </div>
  );
}
