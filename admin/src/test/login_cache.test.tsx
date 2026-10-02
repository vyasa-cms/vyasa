import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { api } from "@/api/client";

vi.mock("@/api/client", () => ({
  api: {
    me: vi.fn(),
    login: vi.fn(),
    loginMfa: vi.fn(),
    logout: vi.fn(),
    setupStatus: vi.fn(),
    listPosts: vi.fn(),
    listCommentsPage: vi.fn(),
    mediaStats: vi.fn(),
    listMediaPage: vi.fn(),
    analyticsSummary: vi.fn(),
    myCaps: vi.fn(),
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

const next = {
  id: 2,
  email: "next@example.com",
  username: "next",
  display_name: "Next",
  role: "subscriber",
  custom_role: "drafter",
  role_name: "Draft writer",
  bio: "",
  created_at: "2026-01-01T00:00:00Z",
};

/** The app at the sign-in page, in a browser the previous user's data is still cached in. */
function makeApp() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // What the last person left behind: their grants (an administrator's),
  // and a list they had open. Their session ended without a sign-out (it
  // expired), so nothing cleared these.
  queryClient.setQueryData(["my-caps"], ["view_admin", "manage_users", "manage_options"]);
  queryClient.setQueryData(["users", "list"], { items: [{ id: 1, email: "secret@example.com" }], total: 1 });
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: ["/admin/login"] }),
    context: { queryClient },
    basepath: "/admin",
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} context={{ queryClient }} />
    </QueryClientProvider>,
  );
  return { queryClient, router };
}

async function signIn() {
  const user = userEvent.setup();
  await user.type(await screen.findByTestId("login-email"), "next@example.com");
  await user.type(screen.getByTestId("login-password"), "pw-secret-1");
  await user.click(screen.getByRole("button", { name: /sign in/i }));
  return user;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.me!.mockResolvedValue(next);
  mocked.setupStatus!.mockResolvedValue({ needs_admin: false });
  mocked.myCaps!.mockResolvedValue(["view_admin", "edit_posts"]);
  mocked.listPosts!.mockResolvedValue({ items: [], total: 0, page: 1, per_page: 20 });
  mocked.listCommentsPage!.mockResolvedValue({ items: [], total: 0 });
  mocked.mediaStats!.mockResolvedValue({ count: 0, bytes: 0, missing_alt: 0, cap_bytes: null });
  mocked.listMediaPage!.mockResolvedValue({ items: [], total: 0 });
  mocked.analyticsSummary!.mockResolvedValue({ today: 0, total: 0, series: [], top_paths: [], top_referrers: [] });
  mocked.version!.mockResolvedValue({ version: "0.0.0", migration_version: null, ui_bundle: null });
});

describe("signing in", () => {
  it("starts from an empty cache, so the last user's data and grants never show for the next", async () => {
    mocked.login!.mockResolvedValue(next);
    // The grants are slow to arrive: whatever the shell shows meanwhile
    // must not be built on the previous user's.
    let arrive: (caps: string[]) => void = () => undefined;
    mocked.myCaps!.mockReturnValue(new Promise<string[]>((r) => { arrive = r; }));
    const { queryClient } = makeApp();
    await signIn();

    await waitFor(() => expect(queryClient.getQueryData(["me"])).toEqual(next));
    expect(queryClient.getQueryData(["users", "list"])).toBeUndefined();
    expect(queryClient.getQueryData(["my-caps"])).toBeUndefined();
    expect(await screen.findByText("Loading permissions…")).toBeInTheDocument();
    // The previous user's navigation (Users, Settings) is not offered.
    expect(screen.queryByRole("link", { name: "Users" })).toBeNull();
    expect(screen.queryByRole("link", { name: "Settings" })).toBeNull();

    arrive(["view_admin", "edit_posts"]);
    expect(await screen.findByRole("heading", { name: "Dashboard" }, { timeout: 3000 })).toBeInTheDocument();
    expect(queryClient.getQueryData(["my-caps"])).toEqual(["view_admin", "edit_posts"]);
    expect(screen.queryByRole("link", { name: "Users" })).toBeNull();
  });

  it("clears the cache when the second factor finishes the sign-in too", async () => {
    mocked.login!.mockResolvedValue({ mfa_required: true, challenge: "c-1" });
    mocked.loginMfa!.mockResolvedValue(next);
    const { queryClient } = makeApp();
    const user = await signIn();
    // The password step alone is not a sign-in: nothing is cleared yet.
    await user.type(await screen.findByTestId("mfa-code"), "123456");
    expect(queryClient.getQueryData(["users", "list"])).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Continue" }));

    await waitFor(() => expect(queryClient.getQueryData(["me"])).toEqual(next));
    expect(queryClient.getQueryData(["users", "list"])).toBeUndefined();
    expect(await screen.findByRole("heading", { name: "Dashboard" }, { timeout: 3000 })).toBeInTheDocument();
    expect(queryClient.getQueryData(["my-caps"])).toEqual(["view_admin", "edit_posts"]);
  });
});
