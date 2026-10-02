import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { Vocabulary } from "@/api/themes";
import { InsertPanel, insertableKinds } from "@/components/studio/InsertPanel";

const vocab: Vocabulary = {
  static_regions: [
    { kind: "header", description: "Site chrome" },
    { kind: "footer", description: "Site chrome, at the bottom" },
  ],
  blocks: [
    {
      kind: "band",
      description: "Full-width band",
      settings_schema: { type: "object" },
      container: true,
      category: "structure",
    },
    {
      kind: "hero",
      description: "Big opening",
      settings_schema: { type: "object" },
      category: "marketing",
      sample: { headline: "A headline" },
      inline: ["headline"],
    },
    {
      kind: "latest-posts",
      description: "Newest entries",
      settings_schema: { type: "object" },
      // No category: the server may be older than the admin, and a kind
      // nothing claims still has to be reachable.
    },
  ],
  templates: ["index"],
  template_files: [],
  token_schema: {},
};

describe("insert library", () => {
  it("flattens the vocabulary, categorising the unclaimed and the chrome", () => {
    const kinds = insertableKinds(vocab);
    expect(kinds.map((k) => [k.kind, k.category])).toEqual([
      ["band", "structure"],
      ["hero", "marketing"],
      ["latest-posts", "content"],
      ["header", "chrome"],
      ["footer", "chrome"],
    ]);
  });

  it("leaves excluded kinds out entirely", () => {
    const kinds = insertableKinds(vocab, ["header", "hero"]);
    expect(kinds.map((k) => k.kind)).toEqual(["band", "latest-posts", "footer"]);
  });

  it("inserts on click and filters by category", async () => {
    const user = userEvent.setup();
    const onInsert = vi.fn();
    render(<InsertPanel vocab={vocab} onInsert={onInsert} />);

    await user.click(screen.getByRole("button", { name: "hero" }));
    expect(onInsert).toHaveBeenCalledWith("hero");

    // The category chips narrow the grid to one group.
    await user.click(screen.getByRole("tab", { name: "Structure" }));
    const panel = screen.getByTestId("insert-panel");
    expect(within(panel).getByRole("button", { name: "band" })).toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "hero" })).toBeNull();
  });
});
