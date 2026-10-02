import * as React from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { api } from "@/api/client";
import { PostChromeSidebar, type ChromeValue } from "@/components/editor/PostChromeSidebar";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { JsonPane } from "@/components/editor/PostEditor";
import { fromLocalInput, toLocalInput } from "@/components/editor/PostChromeSidebar";

describe("JSON pane", () => {
  it("keeps what is typed while the text is not yet valid JSON", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<JsonPane document={{ schema_version: 1, blocks: [] }} error={null} onChange={onChange} />);
    const ta = screen.getByTestId("json-editor") as HTMLTextAreaElement;
    const before = ta.value;
    await user.click(ta);
    await user.keyboard("{Backspace}");
    // The closing brace is gone and stays gone: the pane did not snap back
    // to the pretty-printed document.
    expect(ta.value).toBe(before.slice(0, -1));
    expect(onChange).toHaveBeenLastCalledWith(before.slice(0, -1));
  });

  it("refreshes from the document when it changed somewhere else", () => {
    const { rerender } = render(
      <JsonPane document={{ schema_version: 1, blocks: [] }} error={null} onChange={() => {}} />,
    );
    rerender(
      <JsonPane
        document={{ schema_version: 1, blocks: [{ kind: "paragraph", attrs: { text: "hi" } }] }}
        error={null}
        onChange={() => {}}
      />,
    );
    expect((screen.getByTestId("json-editor") as HTMLTextAreaElement).value).toContain('"hi"');
  });
});

describe("schedule field", () => {
  it("round-trips a local wall-clock time through a UTC instant", () => {
    const local = "2026-09-10T09:30";
    const iso = fromLocalInput(local);
    expect(iso.endsWith("Z")).toBe(true);
    expect(new Date(iso).getTime()).toBe(new Date(local).getTime());
    expect(toLocalInput(iso)).toBe(local);
  });

  it("treats empty and junk as unscheduled", () => {
    expect(fromLocalInput("")).toBe("");
    expect(toLocalInput("")).toBe("");
    expect(toLocalInput("not a date")).toBe("");
  });
});

describe("alt-text nudge", () => {
  it("counts pictures with a file but no description, anywhere in the tree", async () => {
    const { imagesWithoutAlt } = await import("@/components/editor/PostEditor");
    const blocks = [
      { kind: "image", attrs: { url: "/a.png", alt: "" }, children: [] },
      { kind: "image", attrs: { url: "", alt: "" }, children: [] }, // no file yet: not counted
      { kind: "image", attrs: { url: "/b.png", alt: "A bee" }, children: [] },
      {
        kind: "gallery",
        attrs: {},
        children: [{ kind: "image", attrs: { url: "/c.png" }, children: [] }],
      },
    ] as never;
    expect(imagesWithoutAlt(blocks)).toBe(2);
  });
});

describe("classic rich field", () => {
  it("shows stored HTML as markdown and stores what is typed as HTML", async () => {
    const { BlockEditor } = await import("@/components/editor/BlockEditor");
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <BlockEditor
        value={[{ kind: "paragraph", attrs: { text: 'See <a href="/about">us</a>' }, children: [] }]}
        onChange={onChange}
      />,
    );
    const field = screen.getByLabelText("Paragraph text") as HTMLTextAreaElement;
    expect(field.value).toBe("See [us](/about)");
    await user.click(field);
    await user.keyboard(" **now**");
    expect(field.value).toBe("See [us](/about) **now**");
    const last = onChange.mock.calls.at(-1)?.[0];
    expect(last[0].attrs.text).toBe('See <a href="/about">us</a> <strong>now</strong>');
  });
});


describe("post password changes", () => {
  it("keeps an untouched password and distinguishes explicit removal", async () => {
    vi.spyOn(api, "listTerms").mockResolvedValue([]);
    const user = userEvent.setup();
    let current: ChromeValue;
    function Sidebar() {
      const [value, setValue] = React.useState<ChromeValue>({
  slug: "",
  excerpt: "",
  seo_title: "",
  seo_description: "",
  status: "private",
  type: "post",
  password: undefined,
  sticky: false,
  lang: "",
  translation_of: "",
  translations: [],
  scheduled_for: "",
  parent_id: "",
  term_ids: [],
  featured_media_id: "",
  featured_media_url: "",
  featured_blurhash: "",
  featured_focal: "",
  seo_keyphrase: "",
  seo_canonical: "",
  seo_noindex: false,
  seo_nofollow: false,
  og_title: "",
  og_description: "",
  og_image: "",
  schema_type: "",
  product_price: "",
  product_currency: "",
});
      current = value;
      return <PostChromeSidebar value={value} onChange={(patch) => setValue((old) => ({ ...old, ...patch }))} />;
    }
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><Sidebar /></QueryClientProvider>);
    expect(screen.getByLabelText("Password")).toHaveValue("");
    expect(current!.password).toBeUndefined();
    await user.click(screen.getByRole("button", { name: "Remove password" }));
    expect(current!.password).toBe("");
    await user.type(screen.getByLabelText("Password"), "replacement-password");
    expect(current!.password).toBe("replacement-password");
    await user.clear(screen.getByLabelText("Password"));
    expect(current!.password).toBe("");
    vi.restoreAllMocks();
  });
});


describe("site timezone scheduling", () => {
  it("uses the configured zone rather than the browser timezone", () => {
    expect(fromLocalInput("2026-09-10T09:30", "Asia/Kolkata")).toBe("2026-09-10T04:00:00.000Z");
    expect(toLocalInput("2026-09-10T04:00:00Z", "Asia/Kolkata")).toBe("2026-09-10T09:30");
    expect(toLocalInput("2026-01-01T00:30:00Z", "America/Los_Angeles")).toBe("2025-12-31T16:30");
  });
  it("rejects missing DST hours and chooses the first repeated hour", () => {
    expect(fromLocalInput("2026-03-29T02:30", "Europe/Stockholm")).toBe("");
    expect(fromLocalInput("2026-10-25T02:30", "Europe/Stockholm")).toBe("2026-10-25T00:30:00.000Z");
  });
});
