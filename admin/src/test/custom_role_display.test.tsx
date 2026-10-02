import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { api } from "@/api/client";

vi.mock("@/api/client", () => ({
  api: {
    me: vi.fn(),
    login: vi.fn(),
    logout: vi.fn(),
    setupStatus: vi.fn(),
    listPosts: vi.fn(),
    listCommentsPage: vi.fn(),
    mediaStats: vi.fn(),
    listMediaPage: vi.fn(),
    analyticsSummary: vi.fn(),
    myCaps: vi.fn(),
    listApiKeys: vi.fn(),
    mfaStatus: vi.fn(),
    version: vi.fn(),
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

const user = {
  id: 7,
  email: "writer@example.com",
  username: "writer",
  display_name: "Wren",
  // Under a custom role the built-in role is always `subscriber`.
  role: "subscriber",
  custom_role: "drafter",
  role_name: "Draft writer",
  bio: "",
  created_at: "2026-01-01T00:00:00Z",
};

function makeApp(path: string, me: typeof user = user) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [path] }),
    context: { queryClient },
    basepath: "/admin",
  });
  queryClient.setQueryData(["me"], me);
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} context={{ queryClient }} />
    </QueryClientProvider>,
  );
  return { queryClient, router };
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.me!.mockResolvedValue(user);
  mocked.setupStatus!.mockResolvedValue({ needs_admin: false });
  mocked.myCaps!.mockResolvedValue(["view_admin", "edit_posts"]);
  mocked.listPosts!.mockResolvedValue({ items: [], total: 0, page: 1, per_page: 20 });
  mocked.listCommentsPage!.mockResolvedValue({ items: [], total: 0 });
  mocked.mediaStats!.mockResolvedValue({ count: 0, bytes: 0, missing_alt: 0, cap_bytes: null });
  mocked.listMediaPage!.mockResolvedValue({ items: [], total: 0 });
  mocked.analyticsSummary!.mockResolvedValue({ today: 0, total: 0, series: [], top_paths: [], top_referrers: [] });
  mocked.listApiKeys!.mockResolvedValue([]);
  mocked.mfaStatus!.mockResolvedValue({ enabled: false, recovery_codes_left: 0 });
  mocked.version!.mockResolvedValue({ version: "0.0.0", migration_version: null, ui_bundle: null });
});

describe("a custom role is shown by its name", () => {
  it("in the top bar and on the profile page, never as the subscriber it is stored over", async () => {
    makeApp("/admin/profile");
    expect(await screen.findByRole("heading", { name: "Your profile" }, { timeout: 3000 })).toBeInTheDocument();
    expect(screen.getByTestId("topbar-role").textContent).toBe("Draft writer");
    expect(screen.getByText("writer@example.com · Draft writer")).toBeInTheDocument();
    expect(screen.queryByText(/subscriber/i)).toBeNull();
  });

  it("shows a built-in role by its name too", async () => {
    const editor = { ...user, role: "editor", custom_role: null as unknown as string, role_name: "Editor" };
    mocked.me!.mockResolvedValue(editor);
    makeApp("/admin/profile", editor);
    expect(await screen.findByRole("heading", { name: "Your profile" }, { timeout: 3000 })).toBeInTheDocument();
    expect(screen.getByTestId("topbar-role").textContent).toBe("Editor");
    expect(screen.getByText("writer@example.com · Editor")).toBeInTheDocument();
  });
});
