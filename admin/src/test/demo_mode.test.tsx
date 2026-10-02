import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { api } from "@/api/client";
import { DemoBanner } from "@/components/DemoNotice";

vi.mock("@/api/client", () => ({
  api: {
    me: vi.fn(),
    login: vi.fn(),
    setupStatus: vi.fn(),
    registrationInfo: vi.fn(),
    myCaps: vi.fn(),
    version: vi.fn(),
  },
  ApiError: class ApiError extends Error {
    status = 0;
  },
}));

const demo = { email: "demo@vyasa.site", password: "demo" };

function client() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}

function loginPage() {
  const queryClient = client();
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: ["/admin/login"] }),
    context: { queryClient },
    basepath: "/admin",
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.me).mockRejectedValue(new Error("signed out"));
  vi.mocked(api.registrationInfo).mockResolvedValue({ enabled: false, available: false, password_min_length: 12 });
  vi.mocked(api.version).mockResolvedValue({ version: "0.1.0", migration_version: null, ui_bundle: null });
});

describe("demo mode", () => {
  it("shows the shared account on the sign-in page and fills it in", async () => {
    vi.mocked(api.setupStatus).mockResolvedValue({ needs_admin: false, needs_setup: false, step: "done", instance: "x", demo });
    const user = userEvent.setup();
    loginPage();
    expect(await screen.findByText(/demo@vyasa\.site/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /use the demo account/i }));
    expect(screen.getByLabelText(/email/i)).toHaveValue("demo@vyasa.site");
    expect(screen.getByLabelText(/^password/i)).toHaveValue("demo");
  });

  it("says nothing about a demo on an ordinary site", async () => {
    vi.mocked(api.setupStatus).mockResolvedValue({ needs_admin: false, needs_setup: false, step: "done", instance: "x", demo: null });
    loginPage();
    await screen.findByLabelText(/email/i);
    expect(screen.queryByRole("button", { name: /use the demo account/i })).toBeNull();
  });

  it("shows a banner inside the admin of a demo only", async () => {
    vi.mocked(api.setupStatus).mockResolvedValue({ needs_admin: false, needs_setup: false, step: "done", instance: "x", demo });
    const { unmount } = render(
      <QueryClientProvider client={client()}>
        <DemoBanner />
      </QueryClientProvider>,
    );
    expect(await screen.findByText(/resets every hour/i)).toBeInTheDocument();
    unmount();
    vi.mocked(api.setupStatus).mockResolvedValue({ needs_admin: false, needs_setup: false, step: "done", instance: "x", demo: null });
    render(
      <QueryClientProvider client={client()}>
        <DemoBanner />
      </QueryClientProvider>,
    );
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByText(/resets every hour/i)).toBeNull();
  });
});
