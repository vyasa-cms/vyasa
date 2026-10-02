import { describe, expect, it } from "vitest";

import type { Section } from "@/api/themes";
import { moveRelativeTo } from "@/components/studio/sectionTree";

const ids = (sections: Section[]): string => sections.map(shape).join(" ");
const shape = (s: Section): string =>
  s.children === undefined || s.children.length === 0
    ? s.id
    : `${s.id}(${s.children.map(shape).join(" ")})`;

const tree = (): Section[] => [
  { id: "header", kind: "header" },
  {
    id: "band",
    kind: "band",
    children: [
      { id: "hero", kind: "hero" },
      { id: "posts", kind: "latest-posts" },
    ],
  },
  { id: "footer", kind: "footer" },
];

describe("dragging a section to a new place", () => {
  it("reorders among siblings", () => {
    expect(ids(moveRelativeTo(tree(), "footer", "header", "before"))).toBe(
      "footer header band(hero posts)",
    );
  });

  it("lifts a section out of a container", () => {
    // The arrow controls refuse to cross parents on purpose. A drag is an
    // explicit gesture at a specific spot, so it may.
    expect(ids(moveRelativeTo(tree(), "hero", "footer", "after"))).toBe(
      "header band(posts) footer hero",
    );
  });

  it("drops a section into a container", () => {
    expect(ids(moveRelativeTo(tree(), "header", "posts", "before"))).toBe(
      "band(hero header posts) footer",
    );
  });

  it("refuses a move that would detach the subtree", () => {
    // Dropping `band` inside `band` would carry hero and posts out of the
    // tree along with it, and the section would vanish from the page.
    expect(ids(moveRelativeTo(tree(), "band", "hero", "after"))).toBe(ids(tree()));
  });

  it("leaves the tree alone when the move means nothing", () => {
    expect(ids(moveRelativeTo(tree(), "header", "header", "before"))).toBe(ids(tree()));
    expect(ids(moveRelativeTo(tree(), "header", "nope", "after"))).toBe(ids(tree()));
    expect(ids(moveRelativeTo(tree(), "nope", "header", "after"))).toBe(ids(tree()));
  });
});
