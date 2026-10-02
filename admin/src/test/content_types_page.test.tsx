import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { notify } from "@/components/ui/toast";
import { canVisit } from "@/lib/capabilities";
import { Route } from "@/routes/_auth/content-types/index";
import { TestRouter } from "./TestRouter";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: {
      ...original.api,
      listContentTypes: vi.fn(),
      createContentType: vi.fn(),
      updateContentType: vi.fn(),
      deleteContentType: vi.fn(),
      listPosts: vi.fn(),
      myCaps: vi.fn(),
    },
  };
});
vi.mock("@/components/ui/toast", () => ({
  notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), undo: vi.fn() },
}));

const mocked = vi.mocked(api);

/** The dialog focuses itself on the next frame; typing before that would lose keystrokes. */
async function settle() {
  await act(async () => {
    await new Promise((r) => requestAnimationFrame(() => r(undefined)));
  });
}
const Page = Route.options.component as React.ComponentType;

const base = { public: true, has_archive: true, description: "", count: 0 };
const TYPES = [
  { ...base, slug: "post", singular: "Post", plural: "Posts", plugin: false, owner: "builtin", count: 12 },
  { ...base, slug: "page", singular: "Page", plural: "Pages", plugin: false, owner: "builtin", count: 4 },
  { ...base, slug: "book", singular: "Book", plural: "Books", plugin: true, owner: "plugin" },
  { ...base, slug: "product", singular: "Product", plural: "Products", plugin: false, owner: "admin", description: "Things we sell.", count: 3 },
];

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ConfirmProvider>
        <TestRouter>
          <Page />
        </TestRouter>
      </ConfirmProvider>
    </QueryClientProvider>,
  );
}

function rowOf(text: string): HTMLElement {
  return screen.getByText(text).closest("li") as HTMLElement;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.listContentTypes.mockResolvedValue(structuredClone(TYPES) as never);
  mocked.myCaps.mockResolvedValue(["edit_posts", "edit_others", "manage_options", "view_admin"]);
  mocked.listPosts.mockResolvedValue({ items: [], total: 0, page: 1, per_page: 1 } as never);
});

describe("content types page", () => {
  it("is for site managers: the page needs manage_options", () => {
    expect(canVisit("/content-types", ["edit_posts", "view_admin"])).toBe(false);
    expect(canVisit("/content-types", ["manage_options", "view_admin"])).toBe(true);
    expect(canVisit("/content-types/product", ["manage_options"])).toBe(true);
  });

  it("lists built-in and plugin types read-only, and only an administrator's type can be relabelled or deleted", async () => {
    mount();
    await screen.findByText("Products");

    for (const name of ["Posts", "Pages", "Books"]) {
      const row = rowOf(name);
      expect(within(row).queryByRole("button", { name: /Edit labels/ })).not.toBeInTheDocument();
      expect(within(row).queryByRole("button", { name: /Delete/ })).not.toBeInTheDocument();
      // Fields can be added to any type.
      expect(within(row).getByRole("link", { name: /Fields/ })).toBeInTheDocument();
    }
    expect(within(rowOf("Posts")).getByText("Built-in")).toBeInTheDocument();
    expect(within(rowOf("Books")).getByText("Plugin")).toBeInTheDocument();
    expect(within(rowOf("Books")).getByText(/comes from a plugin/i)).toBeInTheDocument();

    const product = rowOf("Products");
    expect(within(product).getByText("Yours")).toBeInTheDocument();
    expect(within(product).getByRole("button", { name: "Edit labels" })).toBeInTheDocument();
    expect(within(product).getByRole("button", { name: "Delete" })).toBeInTheDocument();
    expect(within(product).getByRole("link", { name: /Fields/ })).toHaveAttribute("href", "/content-types/product");
  });

  it("creates a type, stating the slug rules and refusing a bad slug before sending it", async () => {
    const user = userEvent.setup();
    mocked.createContentType.mockResolvedValue({ slug: "event", singular: "Event", plural: "Events", description: "", public: true, has_archive: true, created_at: "", updated_at: "" });
    mount();
    await screen.findByText("Products");

    await user.click(screen.getByRole("button", { name: "New content type" }));
    await settle();
    // The rules are on screen before anything is typed.
    expect(screen.getByText(/2 to 32 characters/)).toBeInTheDocument();
    expect(screen.getByText(/can.t be changed later/i)).toBeInTheDocument();

    const slug = screen.getByLabelText("Slug");
    await user.type(slug, "9lives");
    expect(screen.getByText(/Start with a letter/)).toBeInTheDocument();
    await user.type(screen.getByLabelText("Singular label"), "Event");
    await user.type(screen.getByLabelText("Plural label"), "Events");
    expect(screen.getByRole("button", { name: "Create type" })).toBeDisabled();

    await user.clear(slug);
    await user.type(slug, "a".repeat(33));
    expect(screen.getByRole("button", { name: "Create type" })).toBeDisabled();

    await user.clear(slug);
    await user.type(slug, "event");
    await user.click(screen.getByRole("checkbox", { name: /archive/i }));
    await user.click(screen.getByRole("button", { name: "Create type" }));

    await waitFor(() =>
      expect(mocked.createContentType).toHaveBeenCalledWith({
        slug: "event",
        singular: "Event",
        plural: "Events",
        description: "",
        public: true,
        has_archive: false,
      }),
    );
    expect(vi.mocked(notify).success).toHaveBeenCalledWith("Events created", expect.any(String));
  });

  it("shows the server's reason when the slug is taken", async () => {
    const user = userEvent.setup();
    mocked.createContentType.mockRejectedValue(
      new ApiError(409, { code: "conflict", message: "\"book\" is declared by the plugin \"bookshelf\"" }),
    );
    mount();
    await screen.findByText("Products");
    await user.click(screen.getByRole("button", { name: "New content type" }));
    await settle();
    await user.type(screen.getByLabelText("Slug"), "book");
    await user.type(screen.getByLabelText("Singular label"), "Book");
    await user.type(screen.getByLabelText("Plural label"), "Books");
    await user.click(screen.getByRole("button", { name: "Create type" }));
    await waitFor(() =>
      expect(vi.mocked(notify).error).toHaveBeenCalledWith(
        "Couldn't create the content type",
        expect.objectContaining({ message: "\"book\" is declared by the plugin \"bookshelf\"" }),
      ),
    );
    expect(screen.getByRole("button", { name: "Create type" })).toBeInTheDocument();
  });

  it("relabels without ever sending a new slug", async () => {
    const user = userEvent.setup();
    mocked.updateContentType.mockResolvedValue({ slug: "product", singular: "Item", plural: "Items", description: "Things we sell.", public: true, has_archive: true, created_at: "", updated_at: "" });
    mount();
    await screen.findByText("Products");
    await user.click(within(rowOf("Products")).getByRole("button", { name: "Edit labels" }));
    await settle();

    const slug = screen.getByLabelText("Slug");
    expect(slug).toHaveValue("product");
    expect(slug).toBeDisabled();
    await user.clear(screen.getByLabelText("Singular label"));
    await user.type(screen.getByLabelText("Singular label"), "Item");
    await user.clear(screen.getByLabelText("Plural label"));
    await user.type(screen.getByLabelText("Plural label"), "Items");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(mocked.updateContentType).toHaveBeenCalledWith("product", {
        singular: "Item",
        plural: "Items",
        description: "Things we sell.",
        public: true,
        has_archive: true,
      }),
    );
  });

  it("deletes after confirming, and shows the entry count when the server refuses", async () => {
    const user = userEvent.setup();
    mocked.listPosts.mockResolvedValue({ items: [], total: 5, page: 1, per_page: 1 } as never);
    mocked.deleteContentType.mockRejectedValue(
      new ApiError(409, { code: "conflict", message: "product still has 5 entries (any status, trash included); delete them first" }),
    );
    mount();
    await screen.findByText("Products");
    await user.click(within(rowOf("Products")).getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Delete Products?");
    // The published count would undercount: drafts and trash block it too.
    await waitFor(() => expect(dialog).toHaveTextContent("5 entries (drafts and trash included) must be deleted first"));
    expect(mocked.listPosts).toHaveBeenCalledWith(expect.objectContaining({ type: "product", per_page: 1 }));
    await user.click(screen.getByRole("button", { name: "Delete type" }));

    await waitFor(() => expect(mocked.deleteContentType).toHaveBeenCalledWith("product"));
    // The count is shown on the page, not only in a passing toast.
    expect(await screen.findByRole("alert")).toHaveTextContent("5 entries");
  });

  it("opens the dialog at once and fills in the count when it arrives, one dialog however often Delete is clicked", async () => {
    const user = userEvent.setup();
    let answer: (v: unknown) => void = () => undefined;
    mocked.listPosts.mockReturnValue(new Promise((r) => { answer = r; }) as never);
    mount();
    await screen.findByText("Products");
    const del = within(rowOf("Products")).getByRole("button", { name: "Delete" });
    await user.click(del);
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Delete Products?");
    expect(dialog).toHaveTextContent("Counting its entries");
    // A second click (the dialog is modal, but a fast double-click lands
    // before it) still leaves one dialog.
    del.click();
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    await act(async () => {
      answer({ items: [], total: 2, page: 1, per_page: 1 });
    });
    await waitFor(() => expect(dialog).toHaveTextContent("2 entries (drafts and trash included) must be deleted first"));
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
  });

  it("gives someone who sees only their own entries the general wording, without counting", async () => {
    const user = userEvent.setup();
    mocked.myCaps.mockResolvedValue(["edit_posts", "manage_options", "view_admin"]);
    mount();
    await screen.findByText("Products");
    await user.click(within(rowOf("Products")).getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("the server says how many are left if any");
    expect(mocked.listPosts).not.toHaveBeenCalled();
  });

  it("says a type with no entries at all can go, its fields with it", async () => {
    const user = userEvent.setup();
    mocked.deleteContentType.mockResolvedValue(undefined);
    mount();
    await screen.findByText("Products");
    await user.click(within(rowOf("Products")).getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog).toHaveTextContent("It has no entries"));
    expect(dialog).not.toHaveTextContent("published");
  });
});
