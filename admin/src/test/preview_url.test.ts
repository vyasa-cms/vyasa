import { describe, expect, it } from "vitest";
import { api } from "@/api/client";

describe("preview page url", () => {
  it("turns the token url into the page the server actually serves", () => {
    expect(api.previewPageUrl("/api/v1/posts/42/preview?token=abc")).toBe("/preview/42?token=abc");
  });
  it("leaves anything else alone", () => {
    expect(api.previewPageUrl("/somewhere/else")).toBe("/somewhere/else");
  });
});
