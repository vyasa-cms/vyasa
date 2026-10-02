import { MutationCache, QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { api, ApiError } from "@/api/client";
import {
  clearSessionExpired,
  isSessionExpired,
  safeRedirect,
  sessionErrorHandler,
} from "@/lib/session";

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
  id: 1,
  email: "a@example.com",
  username: "a",
  display_name: "A",
  role: "author",
  bio: "",
  created_at: "2026-01-01T00:00:00Z",
};
const ALL = ["edit_others", "view_admin", "edit_posts", "publish_posts", "upload_media", "manage_options", "moderate_comments"];

function makeApp(path = "/admin") {
  let goToLogin = () => undefined as void;
  const onError = (e: unknown) => handler(e);
  const queryClient: QueryClient = new QueryClient({
    queryCache: new QueryCache({ onError }),
    mutationCache: new MutationCache({ onError }),
    defaultOptions: { queries: { retry: false } },
  });
  const handler = sessionErrorHandler(queryClient, () => goToLogin());
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [path] }),
    context: { queryClient },
    basepath: "/admin",
  });
  goToLogin = () => {
    const back = safeRedirect(router.state.location.href);
    void router.navigate({ to: "/login", search: back === null ? {} : { redirect: back } });
  };
  queryClient.setQueryData(["me"], user);
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} context={{ queryClient }} />
    </QueryClientProvider>,
  );
  return { queryClient, router };
}

beforeEach(() => {
  vi.clearAllMocks();
  clearSessionExpired();
  mocked.me!.mockResolvedValue(user);
  mocked.setupStatus!.mockResolvedValue({ needs_admin: false });
  mocked.myCaps!.mockResolvedValue(ALL);
  mocked.listPosts!.mockResolvedValue({ items: [], total: 2, page: 1, per_page: 20 });
  mocked.listCommentsPage!.mockResolvedValue({ items: [], total: 0 });
  mocked.mediaStats!.mockResolvedValue({ count: 500, bytes: 1000, missing_alt: 40, cap_bytes: null });
  mocked.listMediaPage!.mockResolvedValue({ items: [], total: 3 });
  mocked.analyticsSummary!.mockResolvedValue({ today: 0, total: 0, series: [], top_paths: [], top_referrers: [] });
});

describe("session expiry", () => {
  it("handles only the first 401 after sign-in, and ignores everything else", () => {
    const client = new QueryClient();
    const go = vi.fn();
    const handle = sessionErrorHandler(client, go);
    handle(new ApiError(401, null));
    expect(go).not.toHaveBeenCalled(); // never signed in
    client.setQueryData(["me"], user);
    handle(new ApiError(500, null));
    handle(new Error("offline"));
    expect(go).not.toHaveBeenCalled();
    handle(new ApiError(401, null));
    handle(new ApiError(401, null));
    expect(go).toHaveBeenCalledTimes(1);
    expect(isSessionExpired()).toBe(true);
  });

  it("keeps redirects inside the admin", () => {
    expect(safeRedirect("/posts/12?x=1")).toBe("/posts/12?x=1");
    expect(safeRedirect("//evil.example")).toBeNull();
    expect(safeRedirect("https://evil.example")).toBeNull();
    expect(safeRedirect("/login?redirect=/")).toBeNull();
    expect(safeRedirect(undefined)).toBeNull();
  });

  it("sends a signed-in user whose request comes back 401 to sign in, with the way back", async () => {
    const { router } = makeApp();
    await screen.findByRole("heading", { name: "Dashboard" }, { timeout: 3000 });
    mocked.me!.mockRejectedValue(new ApiError(401, { message: "unauthenticated" }));
    mocked.listPosts!.mockRejectedValue(new ApiError(401, { message: "unauthenticated" }));
    await act(async () => {
      await router.options.context.queryClient.invalidateQueries({ queryKey: ["posts"] });
    });
    expect(await screen.findByTestId("login-email", {}, { timeout: 3000 })).toBeInTheDocument();
    expect(router.state.location.search).toMatchObject({ redirect: "/" });
  });
});

describe("admin shell permissions", () => {
  it("keeps the page when a background capabilities refetch fails", async () => {
    const { queryClient } = makeApp();
    await screen.findByRole("heading", { name: "Dashboard" }, { timeout: 3000 });
    mocked.myCaps!.mockRejectedValue(new Error("blip"));
    await act(async () => {
      await queryClient.refetchQueries({ queryKey: ["my-caps"] });
    });
    expect(screen.getByRole("heading", { name: "Dashboard" })).toBeInTheDocument();
    expect(screen.queryByText("Couldn't load permissions")).toBeNull();
  });
});

describe("dashboard media tile", () => {
  it("counts only an author's own uploads", async () => {
    mocked.myCaps!.mockResolvedValue(["view_admin", "edit_posts", "upload_media"]);
    makeApp();
    await screen.findByRole("heading", { name: "Dashboard" }, { timeout: 3000 });
    await waitFor(() =>
      expect(mocked.listMediaPage).toHaveBeenCalledWith(expect.objectContaining({ owner_id: "1" })),
    );
    expect(mocked.mediaStats).not.toHaveBeenCalled();
    expect(await screen.findByText("Your uploads")).toBeInTheDocument();
    expect(screen.queryByText(/missing alt text/)).toBeNull();
  });

  it("shows library-wide numbers to someone who can see the library", async () => {
    makeApp();
    await screen.findByRole("heading", { name: "Dashboard" }, { timeout: 3000 });
    await waitFor(() => expect(mocked.mediaStats).toHaveBeenCalled());
    expect(mocked.listMediaPage).not.toHaveBeenCalled();
  });
});
