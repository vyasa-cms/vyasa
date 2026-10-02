import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { api } from "@/api/client";
import { staleShell } from "@/routes/_auth";

vi.mock("@/api/client", () => ({
  api: {
    me: vi.fn(),
    login: vi.fn(),
    logout: vi.fn(),
    listPosts: vi.fn(),
    listCommentsPage: vi.fn(),
    mediaStats: vi.fn(),
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

const mockedApi = api as unknown as {
  me: ReturnType<typeof vi.fn>;
  login: ReturnType<typeof vi.fn>;
  listPosts: ReturnType<typeof vi.fn>;
  listCommentsPage: ReturnType<typeof vi.fn>;
  mediaStats: ReturnType<typeof vi.fn>;
  analyticsSummary: ReturnType<typeof vi.fn>;
  myCaps: ReturnType<typeof vi.fn>;
};

const adminUser = {
  id: 1,
  email: "admin@example.com",
  username: "admin",
  display_name: "Admin",
  role: "admin",
  bio: "",
  created_at: "2026-01-01T00:00:00Z",
};

function makeApp() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: ["/admin"] }),
    context: { queryClient },
    basepath: "/admin",
  });
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} context={{ queryClient }} />
    </QueryClientProvider>
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockedApi.myCaps.mockResolvedValue(["edit_others", "view_admin", "edit_posts", "publish_posts", "moderate_comments", "manage_themes", "manage_options", "upload_media", "manage_users", "manage_plugins", "manage_categories"]);
});

describe("admin shell", () => {
  it("redirects unauthenticated users from / to /login", async () => {
    render(makeApp());
    await waitFor(() => {
      expect(screen.getByTestId("login-email")).toBeInTheDocument();
    });
  });

  it("logs in and shows the dashboard", async () => {
    mockedApi.me.mockResolvedValue(adminUser);
    mockedApi.login.mockResolvedValue(adminUser);
    mockedApi.listPosts.mockImplementation(({ status }: { status?: string }) =>
      Promise.resolve({ items: [], total: status === "published" ? 7 : 3, page: 1, per_page: 20 }),
    );
    mockedApi.listCommentsPage.mockResolvedValue({ items: [], total: 151 });
    mockedApi.mediaStats.mockResolvedValue({ count: 203, bytes: 4567, missing_alt: 102 });
    mockedApi.analyticsSummary.mockResolvedValue({
      today: 3,
      total: 42,
      series: [{ day: "2026-09-04", views: 3 }],
      top_paths: [],
      top_referrers: [],
    });

    render(makeApp());

    // Unauthenticated → redirected to login by _auth guard.
    await waitFor(() => {
      expect(screen.getByTestId("login-email")).toBeInTheDocument();
    });

    await userEvent.type(screen.getByTestId("login-email"), "admin@example.com");
    await userEvent.type(screen.getByTestId("login-password"), "pw-secret-1");
    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));
    // Dashboard loads after successful login.
    await screen.findByRole("heading", { name: "Dashboard" }, { timeout: 3000 });
    const values = await screen.findAllByTestId("stat-value");
    // Card order: views, published, draft, pending, media.
    expect(values[0]).toHaveTextContent("42");
    expect(values[1]).toHaveTextContent("7");
    expect(values[3]).toHaveTextContent("151");
    expect(values[4]).toHaveTextContent("203");
    expect(mockedApi.login).toHaveBeenCalledWith("admin@example.com", "pw-secret-1");
  }, 15000);

  it("renders the posts table with rows and pagination", async () => {
    mockedApi.me.mockResolvedValue(adminUser);
    mockedApi.login.mockResolvedValue(adminUser);
    mockedApi.listPosts.mockImplementation((q: { page?: number }) =>
      Promise.resolve({
        items: [
          {
            id: q.page === 2 ? 99 : 42,
            type: "post",
            status: "published",
            slug: "hello-world",
            title: q.page === 2 ? "Page Two" : "Hello World",
            content: {},
            author_id: 1,
            meta: {},
            created_at: "2026-01-01T00:00:00Z",
            updated_at: "2026-01-01T00:00:00Z",
          },
        ],
        total: 21,
        page: q.page ?? 1,
        per_page: 20,
      }),
    );
    mockedApi.listCommentsPage.mockResolvedValue({ items: [], total: 0 });
    mockedApi.mediaStats.mockResolvedValue({ count: 203, bytes: 4567, missing_alt: 102 });
    mockedApi.analyticsSummary.mockResolvedValue({
      today: 3,
      total: 42,
      series: [{ day: "2026-09-04", views: 3 }],
      top_paths: [],
      top_referrers: [],
    });

    render(makeApp());
    const email = await screen.findByTestId("login-email");
    const password = await screen.findByTestId("login-password");
    await userEvent.type(email, "admin@example.com");
    await userEvent.type(password, "pw-secret-1");
    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));

    // Navigate to posts.
    await screen.findByRole("heading", { name: "Dashboard" });
    await userEvent.click(screen.getByRole("link", { name: "Posts" }));

    await waitFor(() => {
      expect(screen.getByTestId("posts-table")).toBeInTheDocument();
    });
    expect(screen.getAllByTestId("post-row").length).toBeGreaterThan(0);
    expect(screen.getByTestId("posts-total")).toHaveTextContent("21 posts");
  }, 15000);

  it("shows an error state when dashboard queries fail", async () => {
    mockedApi.me.mockResolvedValue(adminUser);
    mockedApi.login.mockResolvedValue(adminUser);
    mockedApi.listPosts.mockRejectedValue(new Error("boom"));
    mockedApi.listCommentsPage.mockRejectedValue(new Error("boom"));
    mockedApi.mediaStats.mockRejectedValue(new Error("boom"));

    render(makeApp());
    const email = await screen.findByTestId("login-email");
    const password = await screen.findByTestId("login-password");
    await userEvent.type(email, "admin@example.com");
    await userEvent.type(password, "pw-secret-1");
    await userEvent.click(screen.getByRole("button", { name: /sign in/i }));

    await waitFor(() => {
      expect(screen.getByTestId("query-error")).toBeInTheDocument();
    });
  }, 15000);
});

describe("stale shell detection", () => {
  it("reloads only when the loaded bundle is not the one the server serves", () => {
    // Three deploys in a row looked like they had not landed because a
    // cached index.html kept an old bundle alive while the server version
    // and migration number both read correctly.
    expect(staleShell("index-old.js", "index-new.js")).toBe(true);
    expect(staleShell("index-new.js", "index-new.js")).toBe(false);
  });

  it("never reloads on a guess", () => {
    // An older server does not report a bundle, and a page served some
    // other way has no script to read. Either way: do nothing.
    expect(staleShell(null, "index-new.js")).toBe(false);
    expect(staleShell("index-old.js", null)).toBe(false);
    expect(staleShell("index-old.js", undefined)).toBe(false);
    expect(staleShell(null, null)).toBe(false);
  });
});
