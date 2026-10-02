import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, MessageSquare, ShieldAlert, Trash2 } from "lucide-react";
import { api, type CommentModeration, type CommentResponse } from "@/api/client";
import { Button } from "@/components/ui/button";
import { DataList, type Column } from "@/components/ui/data-list";
import { useConfirm } from "@/components/ui/dialog";
import { Modal } from "@/components/ui/dialog";
import { EmptyState, PageHeader, Chip } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { formatRelative } from "@/routes/_auth/index";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/comments/")({
  component: CommentsPage,
});

type Status = "pending" | "approved" | "spam" | "trash";
type Action = "approve" | "spam" | "trash" | "restore";

const TABS: { value: Status; label: string }[] = [
  { value: "pending", label: "Pending" },
  { value: "approved", label: "Approved" },
  { value: "spam", label: "Spam" },
  { value: "trash", label: "Trash" },
];

export function CommentsPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [page, setPage] = React.useState(1);
  const [status, setStatus] = React.useState<Status>("pending");
  const [selected, setSelected] = React.useState<Set<string | number>>(new Set());
  const [reading, setReading] = React.useState<CommentResponse | null>(null);

  const list = useQuery({
    queryKey: ["comments", "page", status, page, 50],
    queryFn: () => api.listCommentsPage({ status, limit: 50, offset: (page - 1) * 50 }),
  });

  const aiAvailable = useQuery({ queryKey: ["ai-available"], queryFn: () => api.aiAvailable() });
  const screen = useMutation({
    mutationFn: (id: string) => api.screenComment(id),
    onSuccess: () => { void queryClient.invalidateQueries({ queryKey: ["comments"] }); notify.success("Screening completed"); },
    onError: (e) => notify.error("Couldn't screen the comment", e),
  });
  const moderate = useMutation({
    mutationFn: ({ ids, action }: { ids: string[]; action: Action }) =>
      Promise.all(ids.map((id) => api.moderateComment(id, action))),
    onSuccess: (_r, { ids, action }) => {
      setSelected(new Set());
      setReading(null);
      void queryClient.invalidateQueries({ queryKey: ["comments"] });
      const verb =
        action === "approve" ? "approved" : action === "spam" ? "marked as spam" : action === "restore" ? "restored to pending" : "trashed";
      notify.success(
        ids.length === 1 ? `Comment ${verb}` : `${ids.length} comments ${verb}`,
      );
    },
    onError: (e) => notify.error("Couldn't update the comment", e),
  });

  const run = async (ids: string[], action: Action) => {
    if (action === "trash") {
      const ok = await confirm({
        title: ids.length === 1 ? "Move comment to trash?" : `Trash ${ids.length} comments?`,
        description: "Trashed comments stop showing on your site.",
        confirmLabel: "Move to trash",
        destructive: true,
      });
      if (!ok) return;
    }
    moderate.mutate({ ids, action });
  };

  const items = list.data?.items ?? [];
  const total = list.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / 50));
  React.useEffect(() => { if (list.isSuccess && page > totalPages) setPage(totalPages); }, [list.isSuccess, page, totalPages]);

  const columns: Column<CommentResponse>[] = [
    {
      key: "author",
      header: "Author",
      width: "10rem",
      hideBelow: "sm",
      render: (c) => (
        <span className="min-w-0">
          <span className="block truncate font-medium">{c.author_name}</span>
          <span className="block truncate text-[11px] text-muted-foreground">
            {formatRelative(c.created_at)}
          </span>
        </span>
      ),
    },
    {
      key: "content",
      header: "Comment",
      primary: true,
      render: (c) => (
        <button
          type="button"
          onClick={() => setReading(c)}
          className="min-w-0 truncate text-left hover:underline"
        >
          <span className="font-medium sm:hidden">{c.author_name}: </span>
          {c.content}
        </button>
      ),
    },
    {
      key: "screening",
      header: "Screening",
      width: "11rem",
      hideBelow: "md",
      render: (c) => {
        // The column is free JSON: a verdict written by an older build (or
        // by hand) may lack `top`, so nothing here assumes the full shape.
        const m = (c as { moderation?: Partial<CommentModeration> | null }).moderation;
        if (!m || typeof m !== "object")
          return <span className="text-xs text-muted-foreground">—</span>;
        const top = Array.isArray(m.top) ? m.top[0] : undefined;
        return (
          <Chip tone={m.flagged ? "warning" : "success"}>
            {m.flagged
              ? `Flagged${top ? `: ${top.category.replace(/[/_]/g, " ")} ${Math.round(top.score * 100)}%` : ""}`
              : "Looks fine"}
          </Chip>
        );
      },
    },
    {
      key: "post",
      header: "On",
      width: "7rem",
      hideBelow: "md",
      render: (c) => (
        <a
          href={`/admin/posts/${c.post_id}`}
          className="truncate text-muted-foreground hover:text-foreground hover:underline"
        >
          Post #{c.post_id}
        </a>
      ),
    },
  ];

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Comments"
        description={
          status === "pending" && items.length > 0
            ? `${items.length} waiting on you`
            : "Review what readers have left on your posts."
        }
      />

      {/* Status tabs — horizontally scrollable rather than wrapping on phones. */}
      <div
        role="tablist"
        aria-label="Comment status"
        className="scrollbar-thin -mx-3 flex gap-1 overflow-x-auto px-3 sm:mx-0 sm:px-0"
      >
        {TABS.map((tab) => (
          <button
            key={tab.value}
            role="tab"
            aria-selected={status === tab.value}
            onClick={() => {
              setStatus(tab.value);
              setPage(1);
              setSelected(new Set());
            }}
            className={cn(
              "touch-target shrink-0 rounded-md px-3 py-1.5 text-sm transition-colors",
              status === tab.value
                ? "bg-primary text-primary-foreground font-medium"
                : "text-muted-foreground hover:bg-accent hover:text-foreground",
            )}
          >
            {tab.label}
          </button>
        ))}
      </div>

      <DataList
        rows={items}
        columns={columns}
        rowKey={(c) => c.id}
        isLoading={list.isPending}
        error={list.error}
        selection={{
          selected,
          onChange: setSelected,
          bulkActions: (sel) => {
            const ids = [...sel].map(String);
            return (
              <>
                {status === "trash" || status === "spam" ? (
                  <Button size="sm" variant="outline" className="h-7" onClick={() => void run(ids, "restore")}>Restore</Button>
                ) : null}
                {status !== "approved" ? (
                  <Button size="sm" variant="outline" className="h-7" onClick={() => void run(ids, "approve")}>
                    <Check className="h-3.5 w-3.5" aria-hidden="true" />
                    Approve
                  </Button>
                ) : null}
                {status !== "spam" ? (
                  <Button size="sm" variant="outline" className="h-7" onClick={() => void run(ids, "spam")}>
                    <ShieldAlert className="h-3.5 w-3.5" aria-hidden="true" />
                    Spam
                  </Button>
                ) : null}
                <Button size="sm" variant="outline" className="h-7" onClick={() => void run(ids, "trash")}>
                  <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                  Trash
                </Button>
              </>
            );
          },
        }}
        pagination={{ page, totalPages, total, onPage: n => { setPage(n); setSelected(new Set()); }, label: `${total} comments · page ${page} of ${totalPages}` }}
        rowActions={(c) => (
          <>
            {aiAvailable.data?.screening === true ? <Button size="sm" variant="outline" disabled={screen.isPending} onClick={() => screen.mutate(c.id)}>Screen comment</Button> : null}
            {status !== "approved" ? (
              <button
                type="button"
                onClick={() => void run([c.id], "approve")}
                aria-label={`Approve comment by ${c.author_name}`}
                className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-success-subtle hover:text-success"
              >
                <Check className="h-4 w-4" aria-hidden="true" />
              </button>
            ) : null}
            <button
              type="button"
              onClick={() => void run([c.id], "spam")}
              aria-label={`Mark comment by ${c.author_name} as spam`}
              className="hidden h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-warning-subtle hover:text-warning sm:inline-flex"
            >
              <ShieldAlert className="h-4 w-4" aria-hidden="true" />
            </button>
            <button
              type="button"
              onClick={() => void run([c.id], "trash")}
              aria-label={`Move comment by ${c.author_name} to trash`}
              className="inline-flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground hover:bg-destructive-subtle hover:text-destructive"
            >
              <Trash2 className="h-4 w-4" aria-hidden="true" />
            </button>
          </>
        )}
        empty={
          <EmptyState
            icon={MessageSquare}
            title={
              status === "pending"
                ? "Nothing waiting for review"
                : `No ${status} comments`
            }
            description={
              status === "pending"
                ? "New comments will appear here for approval."
                : undefined
            }
          />
        }
      />

      {/* Full comment, because a truncated row is not enough to moderate on. */}
      <Modal
        open={reading !== null}
        onClose={() => setReading(null)}
        title={reading?.author_name ?? ""}
        description={
          reading === null
            ? undefined
            : `On post #${reading.post_id} · ${formatRelative(reading.created_at)}`
        }
        size="lg"
        footer={
          reading === null ? null : (
            <>
              <Button variant="outline" onClick={() => void run([reading.id], "spam")}>
                Mark as spam
              </Button>
              <Button variant="outline" onClick={() => void run([reading.id], "trash")}>
                Move to trash
              </Button>
              <Button onClick={() => void run([reading.id], "approve")}>
                Approve
              </Button>
            </>
          )
        }
      >
        <p className="whitespace-pre-wrap text-sm leading-relaxed">
          {reading?.content}
        </p>
      </Modal>
    </div>
  );
}
