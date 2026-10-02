import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { Route } from "@/routes/_auth/audience/index";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: {
      ...original.api,
      myCaps: vi.fn(),
      analyticsSummary: vi.fn(),
      listSubmissions: vi.fn(),
      listSubscribers: vi.fn(),
      deleteSubmission: vi.fn(),
      deleteSubscriber: vi.fn(),
    },
  };
});
const mocked = vi.mocked(api);
const Page = Route.options.component as React.ComponentType;

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><ConfirmProvider><Page /></ConfirmProvider></QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.analyticsSummary.mockResolvedValue({ today: 1, total: 10, series: [], top_paths: [], top_referrers: [] });
  mocked.listSubmissions.mockResolvedValue([{ id: "s1", form: "contact", name: "Ada", email: "ada@x", message: "Hi", created_at: new Date().toISOString() }]);
  mocked.listSubscribers.mockResolvedValue([{ id: "u1", email: "reader@x", status: "confirmed", created_at: new Date().toISOString(), confirmed_at: new Date().toISOString() }]);
});

describe("audience page", () => {
  it("shows delete controls and deletes with manage_options", async () => {
    mocked.myCaps.mockResolvedValue(["edit_others", "manage_options"]);
    mocked.deleteSubmission.mockResolvedValue(undefined);
    const user = userEvent.setup();
    mount();
    await screen.findByText("Hi");
    await user.click(screen.getByRole("button", { name: "Delete submission from ada@x" }));
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() => expect(mocked.deleteSubmission).toHaveBeenCalledWith("s1"));
  });

  it("hides delete controls for edit_others without manage_options, without hiding the page", async () => {
    mocked.myCaps.mockResolvedValue(["edit_others"]);
    mount();
    await screen.findByText("Hi");
    expect(screen.getByText("reader@x")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete submission from ada@x" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove reader@x" })).not.toBeInTheDocument();
  });
});
