import { TestRouter } from "./TestRouter";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { TaxonomyPage } from "@/routes/_auth/taxonomy/index";
import { SettingsPage } from "@/routes/_auth/settings/index";
import { CommentsPage } from "@/routes/_auth/comments/index";
import { postPath } from "@/lib/permalink";

vi.mock("@/components/editor/MonacoPane", () => ({ MonacoPane: () => null }));
function mount(node: React.ReactNode) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><ConfirmProvider><TestRouter>{node}</TestRouter></ConfirmProvider></QueryClientProvider>);
}
beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(api, "getOptions").mockResolvedValue({ ai_embeddings: true, ai_comment_screening: "flag" });
  vi.spyOn(api, "aiAvailable").mockResolvedValue({ text: true, vision: false, image: false, transcription: false, speech: false, embeddings: true, autofill: true, screening: true });
  vi.spyOn(api, "setupChecks").mockResolvedValue([]);
  vi.spyOn(api, "mailSettings").mockResolvedValue({ host: "", port: 587, username: "", from: "", has_password: false, source: "none", encrypted: true });
});

describe("integrated admin actions", () => {
  it("edits a term and can clear its parent", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "listTerms").mockResolvedValue([
      { id: "1", name: "Parent", slug: "parent", taxonomy: "category", parent_id: null },
      { id: "2", name: "Child", slug: "child", taxonomy: "category", parent_id: "1" },
      { id: "3", name: "Descendant", slug: "descendant", taxonomy: "category", parent_id: "2" },
    ] as never);
    const update = vi.spyOn(api, "updateTerm").mockResolvedValue({});
    mount(<TaxonomyPage />);
    await user.click(await screen.findByRole("button", { name: "Edit Child" }));
    expect(screen.getByLabelText("Name")).toHaveValue("Child");
    await user.clear(screen.getByLabelText("Name"));
    await user.type(screen.getByLabelText("Name"), "Renamed");
    await user.selectOptions(screen.getByLabelText("Parent category"), "");
    expect(screen.queryByRole("option", { name: "Child" })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Descendant" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith("2", { name: "Renamed", slug: "child", parent_id: null }));
  });

  it("queues one explicit AI batch and passes the continuation on the next click", async () => {
    const user = userEvent.setup();
    const backfill = vi.spyOn(api, "backfillAi").mockResolvedValueOnce({ queued: 100, next_after_id: "123" }).mockResolvedValueOnce({ queued: 4, next_after_id: null });
    mount(<SettingsPage />);
    await user.click(await screen.findByRole("button", { name: "Process up to 100 items" }));
    await waitFor(() => expect(backfill).toHaveBeenCalledWith("embeddings", undefined));
    await user.click(await screen.findByRole("button", { name: "Process next batch" }));
    await waitFor(() => expect(backfill).toHaveBeenLastCalledWith("embeddings", "123"));
    expect(await screen.findByText(/4 jobs queued/)).toBeInTheDocument();
    expect(backfill).toHaveBeenCalledTimes(2);
  });

  it("screens an existing comment from the moderation queue", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "listCommentsPage").mockResolvedValue({ items: [{ id: "7", post_id: "2", author_name: "Reader", content: "Check this", status: "pending", created_at: "2026-09-01T12:00:00Z" }], total: 1 } as never);
    const screenComment = vi.spyOn(api, "screenComment").mockResolvedValue({ flagged: false, top: [] } as never);
    mount(<CommentsPage />);
    await user.click(await screen.findByRole("button", { name: "Screen comment" }));
    await waitFor(() => expect(screenComment).toHaveBeenCalledWith("7"));
  });
});

it("previews the configured permalink while preserving page and plugin addresses", () => {
  expect(postPath("post", "hello", "2026-01-01T00:30:00Z", "/{year}/{month}/{slug}")).toBe("/2026/01/hello");
  expect(postPath("page", "about", "2026-01-01T00:30:00Z", "/blog/{slug}")).toBe("/about");
  expect(postPath("book", "rust", "2026-01-01T00:30:00Z", "/blog/{slug}")).toBe("/book/rust");
});
