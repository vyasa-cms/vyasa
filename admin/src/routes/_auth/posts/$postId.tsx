import { createFileRoute } from "@tanstack/react-router";
import { PostEditor } from "@/components/editor/PostEditor";
import { TYPE_SLUG_RE } from "@/lib/fields";

/**
 * One route for writing: `/posts/new` starts a post (`?type=page` starts a
 * page, `?type=product` an entry of that content type) and `/posts/123`
 * edits one. Sharing the route matters — the editor
 * turns a new post into a real draft after the first edit and swaps the URL
 * in place, and a route change here would remount it mid-sentence.
 */
export const Route = createFileRoute("/_auth/posts/$postId")({
  // The writing canvas sets its own measure; the Sections view puts a tree
  // beside a live page and wants the room.
  staticData: { wide: true },
  validateSearch: (search: Record<string, unknown>): { type?: string } =>
    typeof search["type"] === "string" && search["type"] !== "post" && TYPE_SLUG_RE.test(search["type"])
      ? { type: search["type"] }
      : {},
  component: function EditPost() {
    const { postId } = Route.useParams();
    const { type } = Route.useSearch();
    if (postId === "new") {
      return <PostEditor mode={{ kind: "new", type: type ?? "post" }} />;
    }
    // Ids are 64-bit; keep them as the exact string the server sent rather
    // than round-tripping through a lossy JavaScript number.
    if (!/^\d+$/.test(postId)) {
      return <p className="text-sm text-destructive">Invalid post id.</p>;
    }
    return <PostEditor mode={{ kind: "edit", id: postId }} />;
  },
});
