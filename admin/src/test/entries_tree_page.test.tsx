import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "@/api/client";
import { notify } from "@/components/ui/toast";
import { EntryTreeView } from "@/routes/_auth/entries/$type";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: { ...original.api, getPostTree: vi.fn(), movePostTree: vi.fn() },
  };
});
vi.mock("@/components/ui/toast", () => ({
  notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), undo: vi.fn() },
}));

const mocked = vi.mocked(api);

const TREE = {
  type: "guide",
  hierarchical: true,
  truncated: false,
  nodes: [
    { id: "1", parent_id: null, position: 10, title: "Intro", slug: "intro", path: "intro", url: "/guide/intro", status: "published", depth: 1 },
    { id: "2", parent_id: null, position: 20, title: "Install", slug: "install", path: "install", url: "/guide/install", status: "published", depth: 1 },
  ],
};

function mount(client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  render(
    <QueryClientProvider client={client}>
      <EntryTreeView slug="guide" />
    </QueryClientProvider>,
  );
  return client;
}

describe("the entries tree on the entries page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocked.getPostTree.mockResolvedValue(TREE as never);
  });

  it("sends the moves an indent makes and refreshes the tree", async () => {
    mocked.movePostTree.mockResolvedValue(TREE as never);
    const user = userEvent.setup();
    const client = mount();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await user.click(await screen.findByRole("button", { name: "Indent Install" }));
    await waitFor(() => expect(mocked.movePostTree).toHaveBeenCalled());
    const [type, moves] = mocked.movePostTree.mock.calls[0] ?? [];
    expect(type).toBe("guide");
    expect(moves).toEqual([{ id: "2", parent_id: "1", position: 10 }]);
    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ["post-tree", "guide"] }));
    expect(vi.mocked(notify).success).toHaveBeenCalledWith("Moved");
  });

  it("shows the server's rule when a move is refused", async () => {
    mocked.movePostTree.mockRejectedValue(new ApiError(400, { code: "validation", message: "entry 2 would sit 9 levels deep; the limit is 8" }));
    const user = userEvent.setup();
    mount();
    await user.click(await screen.findByRole("button", { name: "Indent Install" }));
    await waitFor(() =>
      expect(vi.mocked(notify).error).toHaveBeenCalledWith(
        "Couldn't move",
        expect.objectContaining({ message: "entry 2 would sit 9 levels deep; the limit is 8" }),
      ),
    );
  });
});
