import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { api, ApiError } from "@/api/client";
import { PostEditor } from "@/components/editor/PostEditor";
import { TestRouter } from "./TestRouter";

const post = {
  id: "12",
  title: "Hello",
  content: { schema_version: 1, blocks: [] },
  excerpt: "",
  slug: "hello",
  status: "draft",
  type: "post",
  updated_at: "2026-09-01T00:00:00Z",
  created_at: "2026-09-01T00:00:00Z",
  layout: [],
  scheduled_for: null,
  parent_id: null,
  meta: {},
  author_id: "1",
};

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <TestRouter>
        <PostEditor mode={{ kind: "edit", id: "12" }} />
      </TestRouter>
    </QueryClientProvider>,
  );
  return client;
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.restoreAllMocks();
  localStorage.setItem("vyasa-editor-mode", "classic");
  vi.spyOn(api, "me").mockResolvedValue({ id: "1" } as never);
  vi.spyOn(api, "myCaps").mockResolvedValue(["edit_posts", "edit_others", "publish_posts"] as never);
  vi.spyOn(api, "getPost").mockResolvedValue(structuredClone(post) as never);
  vi.spyOn(api, "listRevisions").mockResolvedValue([] as never);
  vi.spyOn(api, "postTerms").mockResolvedValue([] as never);
  vi.spyOn(api, "getOptions").mockResolvedValue({} as never);
  vi.spyOn(api, "listTerms").mockResolvedValue([] as never);
  vi.spyOn(api, "lockPost").mockResolvedValue({ mine: true, holder_name: null, seen_ago_secs: null });
  vi.spyOn(api, "unlockPost").mockResolvedValue(undefined);
  vi.spyOn(api, "aiAvailable").mockResolvedValue({ text: false, vision: false, image: false, transcription: false, speech: false });
  vi.spyOn(api, "autosave").mockResolvedValue({});
});

afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
});

describe("post editor saves", () => {
  it("marks only what a save sent as saved, not text typed while it was in flight", async () => {
    const user = userEvent.setup();
    const pending = deferred<unknown>();
    const update = vi.spyOn(api, "updatePost").mockReturnValue(pending.promise as never);
    mount();
    const titleField = await screen.findByPlaceholderText("Post title");
    await waitFor(() => expect(titleField).toHaveValue("Hello"));
    await user.type(titleField, " one");
    await user.click(screen.getAllByRole("button", { name: "Save draft" })[0]!);
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(update.mock.calls[0]?.[1]).toMatchObject({ title: "Hello one" });
    // Typed while the save is on the wire.
    await user.type(titleField, " two");
    await act(async () => {
      pending.resolve({ ...post, title: "Hello one", updated_at: "2026-09-02T00:00:00Z" });
    });
    // " two" was never sent, so it is still unsaved and autosave picks it up.
    expect(await screen.findByText("Unsaved changes")).toBeInTheDocument();
    await waitFor(
      () => expect(vi.mocked(api.autosave)).toHaveBeenCalledWith("12", expect.objectContaining({ title: "Hello one two" })),
      { timeout: 4000 },
    );
  });

  it("keeps the author's text as an autosave before reloading theirs after a conflict", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "updatePost").mockRejectedValue(
      new ApiError(409, { code: "conflict", message: "post changed since you loaded it" }),
    );
    mount();
    const titleField = await screen.findByPlaceholderText("Post title");
    await waitFor(() => expect(titleField).toHaveValue("Hello"));
    const order: string[] = [];
    vi.mocked(api.autosave).mockImplementation(async (_id, body) => {
      order.push(`autosave:${body.title}`);
      return {};
    });
    vi.mocked(api.getPost).mockImplementation(async () => {
      order.push("reload");
      return { ...structuredClone(post), title: "Theirs", updated_at: "2026-09-03T00:00:00Z" } as never;
    });
    vi.mocked(api.listRevisions).mockResolvedValue([
      {
        id: "r9",
        title: "Hello mine",
        content: { schema_version: 1, blocks: [] },
        is_autosave: true,
        created_at: "2026-09-04T00:00:00Z",
      },
    ] as never);
    await user.type(titleField, " mine");
    // Outside a confirm provider the dialog answers "no": reload theirs.
    await user.click(screen.getAllByRole("button", { name: "Save draft" })[0]!);
    await waitFor(() => expect(titleField).toHaveValue("Theirs"));
    expect(order[0]).toBe("autosave:Hello mine");
    expect(order).toContain("reload");
    // The kept text is offered back.
    expect(await screen.findByTestId("draft-offer")).toBeInTheDocument();
  });

  it("stops autosaving after five consecutive failures", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    vi.mocked(api.autosave).mockRejectedValue(new Error("offline"));
    mount();
    const titleField = await screen.findByPlaceholderText("Post title");
    await waitFor(() => expect(titleField).toHaveValue("Hello"));
    await user.type(titleField, "!");
    for (let i = 0; i < 8; i++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(16_000);
      });
    }
    expect(vi.mocked(api.autosave)).toHaveBeenCalledTimes(5);
  });

  it("stops autosaving at once when the session has ended", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    vi.mocked(api.autosave).mockRejectedValue(new ApiError(401, { code: "unauthorized", message: "sign in" }));
    mount();
    const titleField = await screen.findByPlaceholderText("Post title");
    await waitFor(() => expect(titleField).toHaveValue("Hello"));
    await user.type(titleField, "!");
    for (let i = 0; i < 4; i++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(16_000);
      });
    }
    expect(vi.mocked(api.autosave)).toHaveBeenCalledTimes(1);
  });
});
