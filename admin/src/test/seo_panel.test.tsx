import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { SeoPanel } from "@/components/editor/SeoPanel";

vi.mock("@/api/client", async (importOriginal) => {
  const mod = await importOriginal<{ api: Record<string, unknown> }>();
  return {
    ...mod,
    api: {
      ...mod.api,
      seoSignals: vi.fn().mockResolvedValue({ inbound_links: 0, keyphrase_rivals: [{ id: "9", title: "Rival post" }], redirects_here: ["/post/old"] }),
      lastLinkCheck: vi.fn().mockRejectedValue(new Error("never")),
      getMedia: vi.fn(),
    },
  };
});

const chrome = {
  slug: "sourdough-starter", excerpt: "How to keep a sourdough starter alive", seo_title: "", seo_description: "",
  status: "published", type: "post", password: "", sticky: false, lang: "", translation_of: "", translations: [], scheduled_for: "", parent_id: "", term_ids: [],
  featured_media_id: "", featured_media_url: "", featured_blurhash: "", featured_focal: "",
  seo_keyphrase: "sourdough starter", seo_canonical: "", seo_noindex: false, seo_nofollow: false,
  og_title: "", og_description: "", og_image: "", schema_type: "", product_price: "", product_currency: "",
};

describe("seo panel", () => {
  it("scores the draft and shows what the site knows about it", async () => {
    const user = userEvent.setup();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <SeoPanel
          chrome={chrome}
          onChange={vi.fn()}
          title="Sourdough starter care"
          blocks={[{ kind: "paragraph", attrs: { text: "Sourdough starter care is simple. " + "Feed it daily. ".repeat(40) }, children: [] }] as never}
          postId="1"
          isLive
          path="/post/sourdough-starter"
        />
      </QueryClientProvider>,
    );
    expect(screen.getByTestId("seo-summary")).toHaveTextContent(/to consider/);
    await user.click(screen.getByRole("button", { name: /Focus keyphrase/ }));
    expect(screen.getByTestId("keyphrase-findings")).toHaveTextContent("Keyphrase is in the title.");
    expect(await screen.findByTestId("keyphrase-rivals")).toHaveTextContent("Rival post");
    await user.click(screen.getByRole("button", { name: /^Links/ }));
    expect(await screen.findByTestId("inbound-links")).toHaveTextContent("orphan");
  });
});
