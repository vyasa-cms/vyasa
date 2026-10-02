import { describe, expect, it } from "vitest";

import { templateForPath } from "@/lib/preview-template";

describe("which template the canvas is showing", () => {
  // A click on the canvas carries a section id and nothing else, and ids
  // are only unique within one template -- every template has a `header`.
  // The previewed path is what says which `header` was clicked.
  it("maps every path the preview offers", () => {
    expect(templateForPath("/")).toBe("index");
    expect(templateForPath("/post/hello")).toBe("single");
    expect(templateForPath("/about")).toBe("page");
    expect(templateForPath("/search?s=the")).toBe("search");
    expect(templateForPath("/studio-preview-missing-page")).toBe("not-found");
  });

  it("reads archives as archives, however they are addressed", () => {
    expect(templateForPath("/category/rust")).toBe("archive");
    expect(templateForPath("/tag/async")).toBe("archive");
    expect(templateForPath("/archive/2026/08")).toBe("archive");
  });

  it("does not let a query string turn a page into something else", () => {
    // `/about?preview=1` is still the page template; only `/search` keys
    // off the query, and it is matched on the path in front of it.
    expect(templateForPath("/about?preview=1")).toBe("page");
    expect(templateForPath("/?x=1")).toBe("index");
  });
});
