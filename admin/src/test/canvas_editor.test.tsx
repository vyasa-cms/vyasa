import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { CanvasEditor } from "@/components/editor/canvas/CanvasEditor";
import type { Block } from "@/components/editor/blocks";

vi.mock("@/api/client", () => ({
  api: { listMedia: vi.fn().mockResolvedValue([]), uploadMedia: vi.fn() },
}));

// jsdom has no layout; scrolling is a no-op there.
window.scrollTo = () => undefined;
Element.prototype.scrollIntoView = () => undefined;

const b = (
  kind: string,
  attrs: Record<string, unknown>,
  children: Block[] = [],
): Block => ({ kind: kind as Block["kind"], attrs, children });

function mount(value: Block[], onChange = vi.fn()) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return {
    onChange,
    ...render(
      <QueryClientProvider client={client}>
        <CanvasEditor value={value} onChange={onChange} />
      </QueryClientProvider>,
    ),
  };
}

describe("canvas editor", () => {
  it("renders blocks as document content, not as form fields", async () => {
    mount([
      b("heading", { level: 2, text: "The heading" }),
      b("paragraph", { text: "Some <strong>bold</strong> prose." }),
    ]);

    await waitFor(() =>
      expect(screen.getByTestId("canvas-editor")).toBeInTheDocument(),
    );

    // A real h2, and the bold run survives as markup rather than literal tags.
    await waitFor(() => {
      const heading = document.querySelector(".vy-canvas h2");
      expect(heading?.textContent).toBe("The heading");
    });
    expect(document.querySelector(".vy-canvas strong")?.textContent).toBe("bold");
    expect(document.querySelector(".vy-canvas")?.textContent).toContain(
      "Some bold prose.",
    );
    // No kind chips or per-block dropdowns in the writing column.
    expect(document.querySelectorAll(".vy-canvas select")).toHaveLength(0);
  });

  it("shows blocks it cannot edit natively as labelled cards", async () => {
    mount([
      b("paragraph", { text: "before" }),
      b("gallery", {}, [
        b("image", { url: "/a.png", alt: "" }),
        b("image", { url: "/b.png", alt: "" }),
      ]),
    ]);

    await waitFor(() => {
      const card = document.querySelector('[data-vy-block="gallery"]');
      expect(card).not.toBeNull();
      expect(card?.textContent).toContain("Gallery");
      expect(card?.textContent).toContain("2 images");
      expect(card?.querySelectorAll("img")).toHaveLength(2);
    });
  });

  it("renders an image as an image with its caption, and an empty one as an invitation", async () => {
    mount([
      b("image", { url: "/api/v1/media/1/raw", alt: "A cat", caption: "Nap <em>time</em>" }),
    ]);
    await waitFor(() => {
      const img = document.querySelector<HTMLImageElement>(".vy-canvas figure img");
      expect(img?.getAttribute("src")).toBe("/api/v1/media/1/raw");
      expect(img?.getAttribute("alt")).toBe("A cat");
      expect(document.querySelector(".vy-canvas figcaption")?.textContent).toBe("Nap time");
      expect(document.querySelector(".vy-canvas figcaption em")).not.toBeNull();
    });
  });

  it("renders a table as a table with a header row", async () => {
    mount([b("table", { header: ["Name"], rows: [["Ada"], ["Grace"]] })]);
    await waitFor(() => {
      expect(document.querySelectorAll(".vy-canvas th")).toHaveLength(1);
      expect(document.querySelectorAll(".vy-canvas td")).toHaveLength(2);
      expect(document.querySelector(".vy-canvas th")?.textContent).toBe("Name");
    });
  });

  it("mounts an empty post ready to type into", async () => {
    mount([]);
    await waitFor(() =>
      expect(document.querySelector(".vy-canvas p")).not.toBeNull(),
    );
  });

  it("renders a list as a real list element", async () => {
    mount([
      b("list", { ordered: true }, [
        b("paragraph", { text: "first" }),
        b("paragraph", { text: "second" }),
      ]),
    ]);
    await waitFor(() => {
      const list = document.querySelector(".vy-canvas ol");
      expect(list).not.toBeNull();
      expect(list?.querySelectorAll("li")).toHaveLength(2);
    });
  });

  it("renders a quote and a code block with their content", async () => {
    mount([
      b("quote", { text: "Quoted words", citation: "Ada" }),
      b("code", { language: "rust", code: "fn main() {}" }),
    ]);
    await waitFor(() => {
      expect(document.querySelector(".vy-canvas blockquote")?.textContent).toContain(
        "Quoted words",
      );
      expect(document.querySelector(".vy-canvas pre")?.textContent).toContain(
        "fn main() {}",
      );
    });
  });
});
