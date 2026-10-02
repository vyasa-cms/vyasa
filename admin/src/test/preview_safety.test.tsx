import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Section, TokenSet } from "@/api/themes";

const { renderPage } = vi.hoisted(() => ({ renderPage: vi.fn() }));
vi.mock("@/api/client", async (importOriginal) => {
  const mod = await importOriginal<{ api: Record<string, unknown> }>();
  return { ...mod, api: { ...mod.api, renderPage } };
});

import { previewFailure } from "@/api/client";
import { SectionPreview } from "@/components/editor/SectionPreview";
import { PreviewPane } from "@/components/studio/PreviewPane";

const tokens = {
  layout: { breakpoint_sm_px: 640, breakpoint_md_px: 900 },
} as unknown as TokenSet;

function mountSection(client: QueryClient, sections: Section[]) {
  return (
    <QueryClientProvider client={client}>
      <SectionPreview postId="12" sections={sections} document={{ v: 1 }} tokens={tokens} />
    </QueryClientProvider>
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  renderPage.mockResolvedValue("<!doctype html><p>page</p>");
});

describe("preview iframes", () => {
  it("sandbox the page editor preview: same-origin for the canvas, no scripts", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(mountSection(client, [{ id: "a", kind: "hero" }]));
    const frame = await screen.findByTestId("page-preview-frame");
    expect(frame).toHaveAttribute("sandbox", "allow-same-origin");
    expect(frame.getAttribute("sandbox")).not.toMatch(/allow-scripts/);
  });

  it("sandbox the studio preview the same way", () => {
    const client = new QueryClient();
    render(
      <QueryClientProvider client={client}><PreviewPane
        html="<p>x</p><script>parent.alert(1)</script>"
        error={null}
        loading={false}
        path="/"
        onPathChange={() => undefined}
        openUrl="/x"
        title="Draft"
        tokens={tokens}
      /></QueryClientProvider>,
    );
    const frame = screen.getByTestId("preview-frame");
    expect(frame).toHaveAttribute("sandbox", "allow-same-origin");
  });

  it("renders what the query key names, not the newer unsettled props", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const view = render(mountSection(client, [{ id: "a", kind: "hero" }]));
    await waitFor(() => expect(renderPage).toHaveBeenCalledTimes(1));
    // A newer tree arrives but has not settled; a refetch of the settled
    // key must still render the settled tree.
    view.rerender(mountSection(client, [{ id: "b", kind: "cta-band" }]));
    await act(async () => {
      await client.refetchQueries({ queryKey: ["page-preview"] });
    });
    const last = renderPage.mock.calls.at(-1)?.[1] as { sections: Section[] };
    expect(last.sections).toEqual([{ id: "a", kind: "hero" }]);
  });
});

describe("preview failures", () => {
  it("turn an HTML error page into a short message", () => {
    const body = `<!doctype html><html><head><style>body{color:red}</style><title>Oops</title></head><body><h1>Template error</h1><p>${"x".repeat(400)}</p></body></html>`;
    const err = previewFailure(500, body);
    expect(err.status).toBe(500);
    expect(err.message).toMatch(/^Preview failed \(500\): Oops Template error x+…$/);
    expect(err.message).not.toMatch(/</);
    expect(err.message.length).toBeLessThan(220);
  });

  it("use the API's message when the body is a JSON error", () => {
    const err = previewFailure(401, JSON.stringify({ code: "unauthorized", message: "sign in" }));
    expect(err.message).toBe("Preview failed (401): sign in");
    expect(err.code).toBe("unauthorized");
  });
});
