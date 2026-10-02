import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { api } from "@/api/client";
import { canVisit } from "@/lib/capabilities";

vi.mock("@/api/client", () => ({
  api: {
    me: vi.fn(),
    listPosts: vi.fn(),
    listCommentsPage: vi.fn(),
    mediaStats: vi.fn(),
    analyticsSummary: vi.fn(),
    myCaps: vi.fn(),
    version: vi.fn(),
    listContentTypes: vi.fn(),
    restorePost: vi.fn(),
    batchPosts: vi.fn(),
  },
  ApiError: class ApiError extends Error {
    status: number;
    code: string;
    constructor(status: number, body: { message?: string; code?: string } | null) {
      super(body?.message ?? `request failed with ${status}`);
      this.status = status;
      this.code = body?.code ?? "unknown";
    }
  },
}));

const mocked = api as unknown as Record<string, ReturnType<typeof vi.fn>>;

const base = { public: true, has_archive: true, description: "", count: 0 };
const TYPES = [
  { ...base, slug: "post", singular: "Post", plural: "Posts", plugin: false, owner: "builtin" },
  { ...base, slug: "page", singular: "Page", plural: "Pages", plugin: false, owner: "builtin" },
  { ...base, slug: "book", singular: "Book", plural: "Books", plugin: true, owner: "plugin" },
  { ...base, slug: "product", singular: "Product", plural: "Products", plugin: false, owner: "admin" },
];

function makeApp(path: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [path] }),
    context: { queryClient },
    basepath: "/admin",
  });
  queryClient.setQueryData(["me"], { id: "1", display_name: "Admin", role: "admin", role_name: "Administrator" });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} context={{ queryClient }} />
    </QueryClientProvider>,
  );
  return { router };
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.listPosts!.mockResolvedValue({ items: [], total: 0, page: 1, per_page: 20 });
  mocked.listCommentsPage!.mockResolvedValue({ items: [], total: 0 });
  mocked.mediaStats!.mockResolvedValue({ count: 0, bytes: 0, missing_alt: 0, cap_bytes: null });
  mocked.analyticsSummary!.mockResolvedValue({ today: 0, total: 0, series: [], top_paths: [], top_referrers: [] });
  mocked.version!.mockResolvedValue({ version: "0.0.0", migration_version: null, ui_bundle: null });
  mocked.listContentTypes!.mockResolvedValue(TYPES);
});

describe("navigation for content types", () => {
  it("shows each plugin and administrator type with its entries and a way to start one", async () => {
    mocked.myCaps!.mockResolvedValue(["view_admin", "edit_posts", "manage_options"]);
    makeApp("/admin");
    const nav = await screen.findByRole("navigation", { name: "Admin" }, { timeout: 3000 });
    const products = await within(nav).findByRole("link", { name: "Products" });
    expect(products).toHaveAttribute("href", "/admin/entries/product");
    expect(within(nav).getByRole("link", { name: "Books" })).toHaveAttribute("href", "/admin/entries/book");
    expect(within(nav).getByRole("link", { name: "New Product" })).toHaveAttribute("href", "/admin/posts/new?type=product");
    // Posts and pages keep their own entries; they are not listed twice.
    expect(within(nav).getAllByRole("link", { name: "Posts" })).toHaveLength(1);
    // The management page is for site managers.
    expect(within(nav).getByRole("link", { name: "Content types" })).toHaveAttribute("href", "/admin/content-types");
  });

  it("hides the management page from writers, who still get their types' entries", async () => {
    mocked.myCaps!.mockResolvedValue(["view_admin", "edit_posts"]);
    makeApp("/admin");
    const nav = await screen.findByRole("navigation", { name: "Admin" }, { timeout: 3000 });
    await within(nav).findByRole("link", { name: "Products" });
    expect(within(nav).queryByRole("link", { name: "Content types" })).toBeNull();
    expect(canVisit("/entries/product", ["edit_posts"])).toBe(true);
    expect(canVisit("/entries/product", ["view_admin"])).toBe(false);
  });

  it("lists a type's entries and starts a new one in the editor", async () => {
    mocked.myCaps!.mockResolvedValue(["view_admin", "edit_posts", "edit_others"]);
    mocked.listPosts!.mockResolvedValue({
      items: [{ id: "31", title: "Lamp", slug: "lamp", status: "draft", type: "product", updated_at: "2026-09-01T00:00:00Z", public_url: "/product/lamp" }],
      total: 1,
      page: 1,
      per_page: 20,
    });
    makeApp("/admin/entries/product");
    expect(await screen.findByRole("heading", { name: "Products" }, { timeout: 3000 })).toBeInTheDocument();
    await waitFor(() => expect(mocked.listPosts).toHaveBeenCalledWith(expect.objectContaining({ type: "product" })));
    expect((await screen.findAllByRole("link", { name: /Lamp/ }))[0]).toHaveAttribute("href", "/admin/posts/31");
    const main = screen.getByRole("main");
    expect(within(main).getByRole("link", { name: "New product" })).toHaveAttribute("href", "/admin/posts/new?type=product");
  });

  it("asks before moving the selected entries to the trash", async () => {
    const userE = (await import("@testing-library/user-event")).default.setup();
    mocked.myCaps!.mockResolvedValue(["view_admin", "edit_posts", "edit_others"]);
    mocked.batchPosts!.mockResolvedValue({ done: 2, failed: [] });
    const row = (id: string, title: string) => ({ id, title, slug: title.toLowerCase(), status: "draft", type: "product", updated_at: "2026-09-01T00:00:00Z", public_url: null });
    mocked.listPosts!.mockResolvedValue({ items: [row("41", "Lamp"), row("42", "Desk")], total: 2, page: 1, per_page: 20 });
    makeApp("/admin/entries/product");
    await userE.click(await screen.findByRole("checkbox", { name: "Select all rows" }, { timeout: 3000 }));
    await userE.click(screen.getByRole("button", { name: "Move to trash" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Move 2 products to trash?");
    expect(mocked.batchPosts).not.toHaveBeenCalled();
    // Cancelled: nothing moves.
    await userE.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(mocked.batchPosts).not.toHaveBeenCalled();
    // Confirmed: both go.
    await userE.click(screen.getByRole("button", { name: "Move to trash" }));
    await userE.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Move to trash" }));
    await waitFor(() => expect(mocked.batchPosts).toHaveBeenCalledWith(["41", "42"], "trash"));
  });

  it("restores a trashed entry from the list", async () => {
    const userE = (await import("@testing-library/user-event")).default.setup();
    mocked.myCaps!.mockResolvedValue(["view_admin", "edit_posts", "edit_others"]);
    mocked.restorePost!.mockResolvedValue({});
    mocked.listPosts!.mockResolvedValue({
      items: [{ id: "32", title: "Old lamp", slug: "old-lamp", status: "trash", type: "product", updated_at: "2026-09-01T00:00:00Z", public_url: null }],
      total: 1,
      page: 1,
      per_page: 20,
    });
    makeApp("/admin/entries/product");
    await userE.click(await screen.findByRole("button", { name: "Restore Old lamp" }, { timeout: 3000 }));
    await waitFor(() => expect(mocked.restorePost).toHaveBeenCalledWith("32"));
    expect(screen.queryByRole("button", { name: /Move Old lamp to trash/ })).toBeNull();
  });
});
