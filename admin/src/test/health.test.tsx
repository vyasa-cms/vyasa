import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";

import { HealthPage } from "@/routes/_auth/health";

vi.mock("@/api/client", () => ({
  api: { siteHealth: vi.fn() },
}));

const mocked = api as unknown as { siteHealth: ReturnType<typeof vi.fn> };

function makeApp() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={client}>
      <HealthPage />
    </QueryClientProvider>
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("site health", () => {
  it("names each check in plain language and shows what to do", async () => {
    mocked.siteHealth.mockResolvedValue({
      status: "warn",
      checks: [
        { name: "database", status: "ok", detail: "reachable, migration version 21" },
        {
          name: "search_index",
          status: "warn",
          detail: "3 documents indexed but 10 posts published; run `vyasa search reindex`",
        },
      ],
    });
    render(makeApp());

    await waitFor(() => {
      expect(screen.getByText("Database")).toBeInTheDocument();
    });
    // A warning shows twice: under "Needs attention" and in its group.
    expect(screen.getAllByText("Search index").length).toBeGreaterThan(0);
    // The remedy is the reason the page exists, so it must be on screen.
    expect(screen.getAllByText(/vyasa search reindex/).length).toBeGreaterThan(0);
    expect(
      screen.getByText("Running, but some things need attention."),
    ).toBeInTheDocument();
  });

  it("renders a check the UI has no label for rather than dropping it", async () => {
    // A server that gains a check must not have it vanish from the page.
    mocked.siteHealth.mockResolvedValue({
      status: "ok",
      checks: [{ name: "future_check", status: "ok", detail: "fine" }],
    });
    render(makeApp());

    await waitFor(() => {
      expect(screen.getByText("future check")).toBeInTheDocument();
    });
  });

  it("reports a failure to load the report itself", async () => {
    mocked.siteHealth.mockRejectedValue(new Error("network down"));
    render(makeApp());

    await waitFor(() => {
      expect(screen.getByTestId("query-error")).toBeInTheDocument();
    });
    expect(screen.getByText(/network down/)).toBeInTheDocument();
  });
});

describe("site health, grouped and actionable", () => {
  it("puts failures first, groups the rest, and runs a check's remedy", async () => {
    const m = api as unknown as { siteHealth: ReturnType<typeof vi.fn>; siteHealthOp: ReturnType<typeof vi.fn> };
    m.siteHealthOp = vi.fn().mockResolvedValue({ indexed: 3 });
    m.siteHealth.mockResolvedValue({
      status: "fail",
      checked_at: new Date().toISOString(),
      failures: 1,
      warnings: 1,
      checks: [
        { name: "database", status: "ok", detail: "fine", group: "core" },
        { name: "search_index", status: "warn", detail: "behind", group: "content", action: { label: "Rebuild index", op: "reindex" } },
        { name: "scheduled_publisher", status: "fail", detail: "1 post past due", group: "content", action: { label: "Open posts", href: "/admin/posts?status=scheduled" } },
      ],
    });
    render(makeApp());
    const verdict = await screen.findByTestId("health-verdict");
    expect(verdict).toHaveTextContent("Something is broken and needs fixing now.");
    expect(verdict).toHaveTextContent("3 checks · 1 failing · 1 need attention");
    const issues = screen.getByTestId("health-issues");
    const names = Array.from(issues.querySelectorAll("li")).map((li) => li.querySelector("span")?.textContent);
    expect(names).toEqual(["Scheduled publishing", "Search index"]);
    expect(screen.getAllByRole("link", { name: "Open posts" })[0]).toHaveAttribute("href", "/admin/posts?status=scheduled");
    expect(screen.getByText("Core")).toBeInTheDocument();
    const rebuild = screen.getAllByRole("button", { name: "Rebuild index" })[0];
    rebuild?.click();
    await waitFor(() => expect(m.siteHealthOp).toHaveBeenCalledWith("reindex"));
  });
});
