import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { MenusPage } from "@/routes/_auth/menus/index";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    api: {
      ...original.api,
      listMenus: vi.fn(),
      getMenu: vi.fn(),
      updateMenuItem: vi.fn(),
      addMenuItem: vi.fn(),
      deleteMenuItem: vi.fn(),
      deleteMenu: vi.fn(),
    },
  };
});
const mocked = vi.mocked(api);

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ConfirmProvider>
        <MenusPage />
      </ConfirmProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.listMenus.mockResolvedValue([{ id: "m1", slug: "main", name: "Main" }]);
  mocked.getMenu.mockResolvedValue([
    { id: "m1", slug: "main", name: "Main" },
    [{ id: "i1", parent_id: null, label: "Home", url: "/", sort_order: 0 }],
  ]);
  mocked.updateMenuItem.mockResolvedValue({} as never);
});

describe("editing a menu link", () => {
  it("saves once, when editing is finished, with what was actually typed", async () => {
    // A PATCH per keystroke against the cached server value snapped the
    // field back after every character and raced writes computed from the
    // same stale base -- typing two characters saved one of them.
    const user = userEvent.setup();
    mount();
    const label = await screen.findByLabelText("Label for Home");

    await user.type(label, " page");
    expect(label).toHaveValue("Home page");
    expect(mocked.updateMenuItem).not.toHaveBeenCalled();

    await user.tab();
    await waitFor(() => expect(mocked.updateMenuItem).toHaveBeenCalledTimes(1));
    expect(mocked.updateMenuItem).toHaveBeenCalledWith("i1", { label: "Home page" });
  });

  it("commits on Enter and leaves an unchanged value alone", async () => {
    const user = userEvent.setup();
    mount();
    const url = await screen.findByLabelText("Address for Home");

    await user.type(url, "{Enter}");
    expect(mocked.updateMenuItem).not.toHaveBeenCalled();

    await user.type(url, "about{Enter}");
    await waitFor(() => expect(mocked.updateMenuItem).toHaveBeenCalledTimes(1));
    expect(mocked.updateMenuItem).toHaveBeenCalledWith("i1", { url: "/about" });
  });

  it("puts the saved value back on Escape", async () => {
    const user = userEvent.setup();
    mount();
    const label = await screen.findByLabelText("Label for Home");

    await user.type(label, "xyz{Escape}");
    expect(label).toHaveValue("Home");
    expect(mocked.updateMenuItem).not.toHaveBeenCalled();
  });
});


it("renders third-level links and counts every descendant when deleting", async () => {
  mocked.getMenu.mockResolvedValue([{ id: "m1", name: "Main", slug: "main" }, [
    { id: "1", parent_id: null, label: "Top", url: "/", sort_order: 0 },
    { id: "2", parent_id: "1", label: "Child", url: "/child", sort_order: 0 },
    { id: "3", parent_id: "2", label: "Grandchild", url: "/grandchild", sort_order: 0 },
  ]]);
  const user = userEvent.setup(); mount();
  expect(await screen.findByLabelText("Label for Grandchild")).toHaveValue("Grandchild");
  await user.click(screen.getByRole("button", { name: "Remove Top" }));
  expect(screen.getByText("Its 2 nested links are removed too.")).toBeInTheDocument();
});

it("reports list failures instead of offering to create the first menu", async () => {
  mocked.listMenus.mockRejectedValue(new Error("Offline")); mount();
  expect(await screen.findByText("Couldn't load menus")).toBeInTheDocument();
  expect(screen.queryByText("No menus yet")).not.toBeInTheDocument();
});
