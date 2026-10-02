import * as React from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import * as themesApi from "@/api/themes";
import type { Draft, Layout, TokenSet } from "@/api/themes";

vi.mock("@/api/themes", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  getDraft: vi.fn(),
  vocabulary: vi.fn().mockResolvedValue({ blocks: [], sources: [] }),
  applyOps: vi.fn(),
  previewCandidate: vi.fn().mockResolvedValue("<p>preview</p>"),
}));

import { rebaseWorking, useDraft, SYNC_DELAY_MS, type Working } from "@/components/studio/useDraft";

const emptyLayout = Object.fromEntries(
  themesApi.TEMPLATE_TYPES.map((t) => [t, []]),
) as unknown as Layout;

function makeDraft(revision: number, patch: Partial<Draft> = {}): Draft {
  return {
    id: "77",
    name: "Draft",
    base_theme_id: null,
    status: "ready" as Draft["status"],
    status_note: null,
    tokens: { radius_px: 6 } as unknown as TokenSet,
    layout: emptyLayout,
    templates: { "single.html": "<p>base</p>" },
    assets: { css: "", js: "" },
    revision,
    created_at: "",
    updated_at: "",
    warnings: [],
    ...patch,
  };
}

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const hook = renderHook(() => useDraft("77"), { wrapper });
  return { client, ...hook };
}

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(themesApi.getDraft).mockResolvedValue(makeDraft(1));
  vi.mocked(themesApi.applyOps).mockImplementation(async () => makeDraft(9));
});

describe("studio draft sync", () => {
  it("pauses autosave on a remote conflict and, on keep-mine, sends only the sections the author changed", async () => {
    const { client, result } = mount();
    await waitFor(() => expect(result.current.working).not.toBeNull());

    act(() => result.current.setTemplate("single.html", "<p>mine</p>"));
    // The assistant changes tokens and adds a template elsewhere.
    const remote = makeDraft(2, {
      tokens: { radius_px: 12 } as unknown as TokenSet,
      templates: { "single.html": "<p>base</p>", "page.html": "<p>theirs</p>" },
    });
    act(() => client.setQueryData(["theme-draft", "77"], remote));

    await waitFor(() => expect(result.current.conflict?.revision).toBe(2));
    await wait(SYNC_DELAY_MS + 200);
    // No stale whole-section write went out.
    expect(vi.mocked(themesApi.applyOps)).not.toHaveBeenCalled();
    await expect(result.current.flush()).rejects.toThrow(/changed elsewhere/);

    act(() => result.current.keepMine());
    expect(result.current.conflict).toBeNull();
    // Remote changes to untouched sections are adopted.
    expect(result.current.working?.tokens).toEqual({ radius_px: 12 });
    expect(result.current.working?.templates["page.html"]).toBe("<p>theirs</p>");

    await waitFor(() => expect(vi.mocked(themesApi.applyOps)).toHaveBeenCalledTimes(1), { timeout: 3000 });
    expect(vi.mocked(themesApi.applyOps).mock.calls[0]?.[1]).toEqual([
      { op: "set_template", name: "single.html", source: "<p>mine</p>" },
    ]);
  });

  it("take-theirs resets to the remote copy", async () => {
    const { client, result } = mount();
    await waitFor(() => expect(result.current.working).not.toBeNull());
    act(() => result.current.setTemplate("single.html", "<p>mine</p>"));
    act(() => client.setQueryData(["theme-draft", "77"], makeDraft(2, { templates: { "single.html": "<p>theirs</p>" } })));
    await waitFor(() => expect(result.current.conflict).not.toBeNull());
    act(() => result.current.takeTheirs());
    expect(result.current.working?.templates["single.html"]).toBe("<p>theirs</p>");
    expect(result.current.isDirty()).toBe(false);
  });

  it("retries with current changes when the write a caller waited on failed, and never sends one batch twice", async () => {
    const { result } = mount();
    await waitFor(() => expect(result.current.working).not.toBeNull());
    let fail: (e: Error) => void = () => undefined;
    vi.mocked(themesApi.applyOps)
      .mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }))
      .mockImplementation(async () => makeDraft(9));

    act(() => result.current.setTemplate("single.html", "<p>one</p>"));
    let first!: Promise<void>;
    act(() => { first = result.current.flush(); });
    const firstDone = first.catch((e: unknown) => e);
    // Two callers wait on the failing write.
    let a!: Promise<void>;
    let b!: Promise<void>;
    act(() => {
      a = result.current.flush();
      b = result.current.flush();
    });
    await act(async () => {
      fail(new Error("network down"));
      await firstDone;
      await Promise.all([a, b]);
    });
    // One failed attempt, then exactly one retry for both waiters.
    expect(vi.mocked(themesApi.applyOps)).toHaveBeenCalledTimes(2);
    expect(result.current.isDirty()).toBe(false);
  });

  it("rebaseWorking keeps the author's sections and the remote's others", () => {
    const base: Working = { tokens: { a: 1 } as unknown as TokenSet, layout: emptyLayout, templates: { x: "1", y: "1" }, assets: { css: "", js: "" } };
    const mine: Working = { ...base, templates: { x: "mine" } };
    const remote: Working = { ...base, tokens: { a: 2 } as unknown as TokenSet, templates: { x: "1", y: "1", z: "new" }, assets: { css: "r", js: "" } };
    expect(rebaseWorking(base, mine, remote)).toEqual({
      tokens: { a: 2 },
      layout: emptyLayout,
      templates: { x: "mine", z: "new" },
      assets: { css: "r", js: "" },
    });
  });
});
