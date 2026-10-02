import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";
import { FormsPage } from "@/routes/_auth/forms/index";
import { CommentsPage } from "@/routes/_auth/comments/index";
import { ConfirmProvider } from "@/components/ui/dialog";
import { canVisit } from "@/lib/capabilities";

function mount(node: React.ReactNode) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><ConfirmProvider>{node}</ConfirmProvider></QueryClientProvider>);
}
beforeEach(() => { vi.restoreAllMocks(); });

describe("admin review regressions", () => {
  it("keeps delimiters while typing choices and saves separate options", async () => {
    vi.spyOn(api, "listForms").mockResolvedValue([]);
    const create = vi.spyOn(api, "createForm").mockResolvedValue({ slug: "review" } as never);
    const user = userEvent.setup();
    mount(<FormsPage />);
    await user.click(screen.getByRole("button", { name: "New form" }));
    await user.type(screen.getByLabelText("Name", { exact: true }), "Review");
    await user.selectOptions(screen.getAllByLabelText("Field kind")[0]!, "select");
    await user.type(screen.getByLabelText("Choices"), "Red, Blue");
    expect(screen.getByLabelText("Choices")).toHaveValue("Red, Blue");
    await user.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(create).toHaveBeenCalledWith(expect.objectContaining({ fields: expect.arrayContaining([expect.objectContaining({ options: ["Red", "Blue"] })]) })));
  });

  it("shows stored answers after their fields are removed", async () => {
    vi.spyOn(api, "listForms").mockResolvedValue([{ id: "1", name: "Contact", slug: "contact", fields: [], enabled: true, unread: 0 }] as never);
    vi.spyOn(api, "formSubmissions").mockResolvedValue([{ id: "2", created_at: "2026-09-25", read_at: "2026-09-25", data: { old_field: "Historical answer" } }] as never);
    const user = userEvent.setup();
    mount(<FormsPage />);
    await user.click(await screen.findByRole("button", { name: "Inbox" }));
    expect(await screen.findByText("Historical answer")).toBeInTheDocument();
    expect(screen.getByText("old_field (previous field)")).toBeInTheDocument();
  });

  it("does not label an unavailable inbox as empty", async () => {
    vi.spyOn(api, "listForms").mockResolvedValue([{ id: "1", name: "Contact", slug: "contact", fields: [], enabled: true, unread: 0 }] as never);
    vi.spyOn(api, "formSubmissions").mockRejectedValue(new Error("Offline"));
    const user = userEvent.setup(); mount(<FormsPage />);
    await user.click(await screen.findByRole("button", { name: "Inbox" }));
    expect(await screen.findByText("Couldn't load answers")).toBeInTheDocument();
    expect(screen.queryByText("Nothing yet.")).not.toBeInTheDocument();
  });

  it("loads comments beyond the first 50 and resets pagination for a new status", async () => {
    vi.spyOn(api, "aiAvailable").mockResolvedValue({} as never);
    const list = vi.spyOn(api, "listCommentsPage").mockImplementation(async query => ({ items: [{ id: String(query.offset ?? 0), post_id: "3", author_name: "Reader", content: `Comment at ${query.offset}`, created_at: "2026-09-25", status: query.status }] as never, total: 51 }));
    const user = userEvent.setup(); mount(<CommentsPage />);
    await screen.findByText("Comment at 0");
    await user.click(screen.getByRole("button", { name: /next/i }));
    expect(await screen.findByText("Comment at 50")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Approved" }));
    await waitFor(() => expect(list).toHaveBeenLastCalledWith({ status: "approved", limit: 50, offset: 0 }));
  });

  it("requires effective grants for nested admin routes", () => {
    expect(canVisit("/admin/appearance/studio/77", ["edit_posts"])).toBe(false);
    expect(canVisit("/admin/appearance/studio/77", ["manage_themes"])).toBe(true);
    expect(canVisit("/users", ["view_admin"])).toBe(false);
    expect(canVisit("/privacy", ["manage_users"])).toBe(false);
    expect(canVisit("/profile", ["view_admin"])).toBe(true);
    expect(canVisit("/posts", ["edit_posts"])).toBe(true);
  });

  // docs/ROUTE-ACCESS.md is the source of truth for what each page's own
  // routes actually require; these three were out of step with it.
  it("matches docs/ROUTE-ACCESS.md for webhooks, audience and media", () => {
    // GET /webhooks (and every other webhooks route) takes manage_plugins.
    expect(canVisit("/webhooks", ["manage_options"])).toBe(false);
    expect(canVisit("/webhooks", ["manage_plugins"])).toBe(true);

    // GET /analytics/summary, /audience/submissions and /audience/subscribers take edit_others.
    expect(canVisit("/audience", ["manage_options"])).toBe(false);
    expect(canVisit("/audience", ["edit_others"])).toBe(true);

    // GET /media (and /media/stats, /media/{id}, ...) takes upload_media or edit_posts.
    expect(canVisit("/media", ["upload_media"])).toBe(true);
    expect(canVisit("/media", ["edit_posts"])).toBe(true);
    expect(canVisit("/media", ["edit_others"])).toBe(false);
  });
});
