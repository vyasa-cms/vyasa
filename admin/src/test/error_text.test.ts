import { describe, expect, it } from "vitest";

import { errorSummary, errorText } from "@/lib/error-text";

// The body that actually reached the screen, cut off mid-word in a toast,
// a drafts list and a studio panel.
const OPENROUTER = `external service error (openrouter): transport: all models failed: openrouter/minimax/minimax-m3:free: transport: read: error decoding response body | openrouter/google/gemma-4-31b-it:free: status 429: {"error":{"message":"Provider returned error","code":429,"metadata":{"raw":"google/gemma-4-31b-it:free is temporarily rate-limited upstream. Please retry shortly, or add your own key"}}}`;

describe("showing an error to a person", () => {
  it("reads a message off whatever was thrown", () => {
    expect(errorText(new Error("Rate limited by the provider."))).toBe(
      "Rate limited by the provider.",
    );
    expect(errorText("plain string")).toBe("plain string");
  });

  it("flattens the whitespace a JSON body drags in", () => {
    expect(errorText(new Error("line one\n\n  line   two\t"))).toBe("line one line two");
  });

  it("leaves a message that already fits alone", () => {
    const short = "Rate limited by the provider. Try again shortly.";
    expect(errorSummary(short)).toEqual({ summary: short, rest: null });
  });

  it("cuts a long one somewhere deliberate and keeps the whole thing", () => {
    const { summary, rest } = errorSummary(OPENROUTER);
    expect(summary.length).toBeLessThan(200);
    expect(summary.endsWith("…")).toBe(true);
    // Never mid-word: the character before the ellipsis closes something.
    expect(summary.at(-2)).not.toBe(" ");
    expect(/[\w.:)}\]]$/.test(summary.slice(0, -1))).toBe(true);
    // Details get the full text, not the leftover tail.
    expect(rest).toBe(errorText(OPENROUTER));
  });

  it("does not invent an ellipsis for a message that is merely long-ish", () => {
    const exact = "x".repeat(180);
    expect(errorSummary(exact).rest).toBeNull();
    expect(errorSummary(`${exact}yz`).rest).not.toBeNull();
  });
});
