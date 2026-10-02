import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { api } from "@/api/client";
import { CAPABILITIES } from "@/lib/capabilities";

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
  email: "member@example.com",
  username: "member",
  display_name: "Member",
  role: "subscriber",
  custom_role: "member",
  role_name: "Member",
  bio: "",
  created_at: "2026-01-01T00:00:00Z",
};

function makeApp(path: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [path] }),
    context: { queryClient },
    basepath: "/admin",
  });
  queryClient.setQueryData(["me"], user);
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} context={{ queryClient }} />
    </QueryClientProvider>,
  );
  return { queryClient, router };
}

const NOTE = /can sign in and manage your own profile, but can.t use the admin screens/;

beforeEach(() => {
  vi.clearAllMocks();
  mocked.me!.mockResolvedValue(user);
  mocked.setupStatus!.mockResolvedValue({ needs_admin: false });
  mocked.listPosts!.mockResolvedValue({ items: [], total: 0, page: 1, per_page: 20 });
  mocked.listCommentsPage!.mockResolvedValue({ items: [], total: 0 });
  mocked.mediaStats!.mockResolvedValue({ count: 0, bytes: 0, missing_alt: 0, cap_bytes: null });
  mocked.listMediaPage!.mockResolvedValue({ items: [], total: 0 });
  mocked.analyticsSummary!.mockResolvedValue({ today: 0, total: 0, series: [], top_paths: [], top_referrers: [] });
  mocked.listApiKeys!.mockResolvedValue([]);
  mocked.mfaStatus!.mockResolvedValue({ enabled: false, recovery_codes_left: 0 });
  mocked.version!.mockResolvedValue({ version: "0.0.0", migration_version: null, ui_bundle: null });
});

describe("the admin without view_admin", () => {
  it.each([
    ["/admin", "the dashboard"],
    ["/admin/posts", "a page their other capabilities would open"],
    ["/admin/settings", "a page they hold nothing for"],
  ])("sends %s (%s) to the profile page, with an explanation and no navigation", async (path) => {
    // Holds a capability, but not the one that opens the admin screens.
    mocked.myCaps!.mockResolvedValue(["edit_posts"]);
    const { router } = makeApp(path);

    expect(await screen.findByRole("heading", { name: "Your profile" }, { timeout: 3000 })).toBeInTheDocument();
    expect(router.state.location.pathname).toBe("/profile");
    expect(screen.getByTestId("profile-only-note").textContent).toMatch(NOTE);
    // Sign-out stays; the navigation, and the way to open it, are gone.
    expect(screen.getByTestId("logout-button")).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Admin" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Open navigation" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Dashboard" })).toBeNull();
    // Nothing behind the admin screens was asked for.
    expect(mocked.listPosts).not.toHaveBeenCalled();
    expect(mocked.analyticsSummary).not.toHaveBeenCalled();
  });

  it("opens the profile page directly", async () => {
    mocked.myCaps!.mockResolvedValue([]);
    const { router } = makeApp("/admin/profile");
    expect(await screen.findByRole("heading", { name: "Your profile" }, { timeout: 3000 })).toBeInTheDocument();
    expect(router.state.location.pathname).toBe("/profile");
    expect(screen.getByTestId("profile-only-note")).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Admin" })).toBeNull();
  });

  it("leaves someone who holds view_admin alone", async () => {
    mocked.myCaps!.mockResolvedValue(["view_admin", "edit_posts"]);
    const { router } = makeApp("/admin");
    expect(await screen.findByRole("heading", { name: "Dashboard" }, { timeout: 3000 })).toBeInTheDocument();
    expect(router.state.location.pathname).toBe("/");
    expect(screen.getByRole("navigation", { name: "Admin" })).toBeInTheDocument();
    expect(screen.queryByTestId("profile-only-note")).toBeNull();
  });

  it("does not decide before the capabilities have loaded", async () => {
    let resolve: (caps: string[]) => void = () => undefined;
    mocked.myCaps!.mockReturnValue(new Promise<string[]>((r) => { resolve = r; }));
    const { router } = makeApp("/admin");
    expect(await screen.findByText("Loading permissions…")).toBeInTheDocument();
    expect(router.state.location.pathname).toBe("/");
    resolve(["view_admin"]);
    await waitFor(() => expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument(), { timeout: 3000 });
    expect(router.state.location.pathname).toBe("/");
  });

  it("says the same thing in the role editor's capability description", () => {
    expect(CAPABILITIES.view_admin!.description).toMatch(/can sign in and manage their own profile, but can.t use the admin screens/);
  });
});
