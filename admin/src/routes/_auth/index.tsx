import { useMe } from "@/components/auth";
import { useCapabilities } from "@/lib/capabilities";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import {
  BarChart3,
  FileText,
  Image as ImageIcon,
  MessageSquare,
  Palette,
  PenLine,
  PlusCircle,
} from "lucide-react";
import { api } from "@/api/client";
import { QueryBoundary } from "@/components/auth";
import { buttonVariants } from "@/components/ui/button";
import {
  Chip,
  EmptyState,
  ErrorNote,
  PageHeader,
  Panel,
  Skeleton,
  StatusChip,
} from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/")({
  component: DashboardPage,
});

/**
 * One tile resolves independently: a slow media query no longer blanks the
 * counts that already arrived.
 */
function StatCard({
  label,
  value,
  hint,
  icon: Icon,
  to,
  tone = "default",
  isLoading,
  isError,
}: {
  label: string;
  value: string | number;
  hint?: string;
  icon: typeof FileText;
  to?: string;
  tone?: "default" | "attention";
  isLoading: boolean;
  isError: boolean;
}) {
  const body = (
    <div
      data-testid={`stat-${label}`}
      className={cn(
        "flex h-full flex-col gap-1 rounded-lg border p-3 text-left transition-colors sm:p-4",
        tone === "attention"
          ? "border-warning/40 bg-warning-subtle"
          : "bg-card hover:border-input",
      )}
    >
      <div className="flex items-center gap-1.5">
        <Icon
          className={cn(
            "h-3.5 w-3.5",
            tone === "attention" ? "text-warning" : "text-muted-foreground",
          )}
          aria-hidden="true"
        />
        <span
          className={cn(
            "text-[10px] font-semibold uppercase tracking-widest",
            tone === "attention" ? "text-warning" : "text-muted-foreground",
          )}
        >
          {label}
        </span>
      </div>

      {isLoading ? (
        <Skeleton className="mt-1 h-7 w-12" />
      ) : (
        <span
          data-testid="stat-value"
          className={cn(
            "text-2xl font-semibold tabular-nums tracking-tight sm:text-3xl",
            tone === "attention" && "text-warning",
          )}
        >
          {isError ? "—" : value}
        </span>
      )}

      {hint !== undefined ? (
        <span className="text-xs text-muted-foreground">{hint}</span>
      ) : null}
    </div>
  );

  return to === undefined ? (
    body
  ) : (
    <Link to={to} className="block h-full rounded-lg">
      {body}
    </Link>
  );
}

function DashboardPage() {
  const caps = useCapabilities();
  const { can } = caps;
  const me = useMe();
  const author = can("edit_others") ? undefined : me.data?.id;
  const postsPublished = useQuery({
    queryKey: ["posts", "published", "post", 1, author],
    enabled: can("edit_posts"),
    queryFn: () => api.listPosts({ type: "post", author_id: author, status: "published", per_page: 1, page: 1 }),
  });
  const postsDraft = useQuery({
    queryKey: ["posts", "draft", "post", 1, author],
    enabled: can("edit_posts"),
    queryFn: () => api.listPosts({ type: "post", author_id: author, status: "draft", per_page: 1, page: 1 }),
  });
  const commentsPending = useQuery({
    queryKey: ["comments", "count", "pending"],
    enabled: can("moderate_comments"),
    queryFn: () => api.listCommentsPage({ status: "pending", limit: 1 }),
  });
  // The media page shows someone without edit_others only their own
  // uploads; the dashboard must not count the whole library for them.
  // `/media/stats` has no owner filter, so their tile counts through the
  // owner-filtered list instead, and the library-wide size and alt-text
  // figures are left to people who can see the library.
  const libraryWide = can("edit_others");
  const media = useQuery({
    queryKey: ["media", "stats"],
    enabled: can("upload_media") && libraryWide,
    queryFn: () => api.mediaStats(),
  });
  const mediaOwner = me.data?.id === undefined ? undefined : String(me.data.id);
  const ownMedia = useQuery({
    queryKey: ["media", "count", "owner", mediaOwner],
    enabled: can("upload_media") && !libraryWide && mediaOwner !== undefined,
    queryFn: () => api.listMediaPage({ limit: 1, offset: 0, owner_id: mediaOwner }),
  });
  const recentDrafts = useQuery({
    queryKey: ["posts", "recent-drafts", "post", author],
    enabled: can("edit_posts"),
    queryFn: () => api.listPosts({ type: "post", author_id: author, status: "draft", per_page: 5, page: 1 }),
  });
  const views = useQuery({
    queryKey: ["analytics", 7],
    enabled: can("manage_options"),
    queryFn: () => api.analyticsSummary(7),
  });

  // Only a total failure is worth replacing the page; a single dead query
  // shows an em dash in its own tile.
  const allFailed =
    postsPublished.isError &&
    postsDraft.isError &&
    commentsPending.isError &&
    media.isError;

  if (allFailed) {
    return (
      <div className="space-y-4">
        <PageHeader title="Dashboard" />
        <ErrorNote
          title="Couldn't load your dashboard"
          error={postsPublished.error ?? new Error("request failed")}
        />
      </div>
    );
  }

  if (caps.isSuccess && !can("edit_posts") && !can("manage_options")) {
    return <div className="space-y-4"><PageHeader title="Dashboard" description="Manage your account and sign-in settings." /><Link to="/profile" className={buttonVariants({ variant: "outline" })}>Your profile</Link></div>;
  }

  const missingAlt = media.data?.missing_alt ?? 0;
  const totalBytes = media.data?.bytes ?? 0;
  const pendingCount = commentsPending.data?.total ?? 0;

  return (
    <div className="space-y-5 sm:space-y-6">
      <PageHeader
        title="Dashboard"
        description="What's live, what's waiting, and what to pick back up."
        actions={
          <>
            {/* The studio is the other daily door; writing and designing
                sit side by side, the way the product now works. */}
            {can("manage_themes") ? <Link
              to="/appearance"
              className={cn(buttonVariants({ size: "sm", variant: "outline" }))}
              data-testid="design-cta"
            >
              <Palette className="h-4 w-4" aria-hidden="true" />
              Design
            </Link> : null}
            {can("edit_posts") ? <Link
              to="/posts/$postId" params={{ postId: "new" }}
              className={cn(buttonVariants({ size: "sm" }))}
              data-testid="new-post-cta"
            >
              <PlusCircle className="h-4 w-4" aria-hidden="true" />
              Write a post
            </Link> : null}
          </>
        }
      />

      <div className="grid grid-cols-2 gap-3 lg:grid-cols-5">
        {can("manage_options") ? <StatCard
          label="views"
          value={views.data?.total ?? 0}
          hint={
            views.data !== undefined && views.data.today > 0
              ? `Last 7 days · ${views.data.today} today`
              : "Last 7 days, cookieless"
          }
          icon={BarChart3}
          to="/audience"
          isLoading={views.isPending}
          isError={views.isError}
        /> : null}
        <StatCard
          label="published"
          value={postsPublished.data?.total ?? 0}
          hint="Live on the site"
          icon={FileText}
          to="/posts"
          isLoading={postsPublished.isPending}
          isError={postsPublished.isError}
        />
        <StatCard
          label="draft"
          value={postsDraft.data?.total ?? 0}
          hint="Not published yet"
          icon={PenLine}
          to="/posts"
          isLoading={postsDraft.isPending}
          isError={postsDraft.isError}
        />
        {can("moderate_comments") ? <StatCard
          label="pending"
          value={pendingCount}
          hint={
            pendingCount > 0 ? "Comments awaiting review" : "Nothing to moderate"
          }
          icon={MessageSquare}
          to="/comments"
          tone={pendingCount > 0 ? "attention" : "default"}
          isLoading={commentsPending.isPending}
          isError={commentsPending.isError}
        /> : null}
        {can("upload_media") && !libraryWide ? <StatCard
          label="media"
          value={ownMedia.data?.total ?? 0}
          hint="Your uploads"
          icon={ImageIcon}
          to="/media"
          isLoading={ownMedia.isPending}
          isError={ownMedia.isError}
        /> : null}
        {can("upload_media") && libraryWide ? <StatCard
          label="media"
          value={media.data?.count ?? 0}
          hint={
            missingAlt > 0
              ? `${formatBytes(totalBytes)} · ${missingAlt} missing alt text`
              : formatBytes(totalBytes)
          }
          icon={ImageIcon}
          to="/media"
          isLoading={media.isPending}
          isError={media.isError}
        /> : null}
      </div>

      <div className="grid gap-4 lg:grid-cols-[1.4fr_1fr]">
        <Panel
          title="Pick up where you left off"
          description="Your most recent drafts."
        >
          <DraftList query={recentDrafts} />
        </Panel>

        <Panel title="Site health">
          <ul className="divide-y text-sm">
            <HealthRow
              label="Content"
              value={`${postsPublished.data?.total ?? 0} published`}
              ok={!postsPublished.isError}
            />
            {can("moderate_comments") ? <HealthRow
              label="Moderation queue"
              value={pendingCount === 0 ? "Clear" : `${pendingCount} waiting`}
              ok={pendingCount === 0}
            /> : null}
            {can("upload_media") && libraryWide ? <HealthRow
              label="Media library"
              value={
                missingAlt === 0
                  ? "All images described"
                  : `${missingAlt} missing alt text`
              }
              ok={missingAlt === 0}
            /> : null}
            <HealthRow
              label="Admin session"
              value="Signed in"
              ok
            />
          </ul>
        </Panel>
      </div>
    </div>
  );
}

function HealthRow({
  label,
  value,
  ok,
}: {
  label: string;
  value: string;
  ok: boolean;
}) {
  return (
    <li className="flex items-center justify-between gap-3 py-2 first:pt-0 last:pb-0">
      <span className="text-muted-foreground">{label}</span>
      <Chip tone={ok ? "success" : "warning"}>{value}</Chip>
    </li>
  );
}

function DraftList({
  query,
}: {
  query: UseQueryResult<Awaited<ReturnType<typeof api.listPosts>>, Error>;
}) {
  return (
    <QueryBoundary
      query={query}
      empty={
        <EmptyState
          icon={PenLine}
          title="No drafts in progress"
          description="Everything you've started is published."
        />
      }
    >
      {(data) =>
        data.items.length === 0 ? (
          <EmptyState
            icon={PenLine}
            title="No drafts in progress"
            description="Everything you've started is published."
            action={
              <Link
                to="/posts/$postId" params={{ postId: "new" }}
                className={cn(buttonVariants({ size: "sm", variant: "outline" }))}
              >
                Start a new post
              </Link>
            }
          />
        ) : (
          <ul className="divide-y">
            {data.items.map((post) => (
              <li key={post.id}>
                <Link
                  to="/posts/$postId"
                  params={{ postId: String(post.id) }}
                  className="flex items-center gap-3 py-2.5 first:pt-0 hover:opacity-80"
                >
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">
                      {post.title || "(untitled)"}
                    </span>
                    <span className="block truncate text-xs text-muted-foreground">
                      Edited {formatRelative(post.updated_at)}
                    </span>
                  </span>
                  <StatusChip status={post.status} />
                </Link>
              </li>
            ))}
          </ul>
        )
      }
    </QueryBoundary>
  );
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Relative time without pulling in a date library. */
export function formatRelative(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "recently";
  const seconds = Math.round((Date.now() - then) / 1000);
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days === 1) return "yesterday";
  if (days < 30) return `${days} days ago`;
  return new Date(iso).toLocaleDateString();
}

// Re-exported for tests that want to exercise the boundary states.
export { QueryBoundary };
