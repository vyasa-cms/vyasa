import * as React from "react";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import { describe, it, expect, vi, afterEach } from "vitest";
import { ItemsEditor } from "@/components/studio/ItemsEditor";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
const runtime = readFileSync(resolve(process.cwd(), "../crates/themes/src/components.js"), "utf8");

function enhance() { new Function(runtime)(); }
function example(title: string, code: string) {
  return `<section class="vy-code-example"><h3>${title}</h3><pre><code>${code}</code></pre></section>`;
}
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); document.body.innerHTML = ""; });

describe("product-site components", () => {
  it("keeps multiline examples and enum fields editable", () => {
    function Editor() {
      const [items, setItems] = React.useState([{ title: "Python", code: "print(1)\nprint(2)", status: "preview" }]);
      return <ItemsEditor label="Examples" schema={{ type: "array", items: { properties: { title: { type: "string" }, code: { type: "string", format: "multiline" }, status: { type: "string", enum: ["available", "preview"] } } } }} items={items} onChange={(next) => setItems(next as typeof items)} />;
    }
    render(<Editor />);
    const code = screen.getByLabelText("Item 1 code");
    expect(code.tagName).toBe("TEXTAREA");
    fireEvent.change(code, { target: { value: "first\nsecond" } });
    expect(code).toHaveValue("first\nsecond");
    fireEvent.change(screen.getByLabelText("Item 1 status"), { target: { value: "available" } });
    expect(screen.getByLabelText("Item 1 status")).toHaveValue("available");
  });
  it("supports independent tab sets, keyboard navigation, and readable fallbacks", () => {
    document.body.innerHTML = [1, 2].map(() => `<div class="vy-code-tabs"><h2>Examples</h2><div class="vy-code-examples">${example("CLI", "echo hello")}${example("Python", "print(42)")}</div></div>`).join("");
    expect(document.querySelectorAll("[hidden]")).toHaveLength(0);
    enhance();
    const groups = screen.getAllByRole("tablist");
    if (!groups[0] || !groups[1]) throw new Error("Expected two independent tab groups");
    const first = within(groups[0]).getAllByRole("tab");
    if (!first[0] || !first[1]) throw new Error("Expected CLI and Python tabs");
    first[0].focus();
    fireEvent.keyDown(first[0], { key: "ArrowRight" });
    expect(first[1]).toHaveFocus();
    expect(first[1]).toHaveAttribute("aria-selected", "true");
    expect(within(groups[1]).getAllByRole("tab")[0]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(first[1], { key: "Home" });
    expect(first[0]).toHaveFocus();
    const ids = Array.from(document.querySelectorAll("[id]"), (node) => node.id);
    expect(new Set(ids).size).toBe(ids.length);
    enhance();
    expect(screen.getAllByRole("tablist")).toHaveLength(2);
  });
  it("copies literal code and announces clipboard failures", async () => {
    const writeText = vi.fn().mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("denied"));
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    document.body.innerHTML = `<div class="vy-code-tabs"><div class="vy-code-examples">${example("HTML", "&lt;hello&gt;\nworld")}</div></div>`;
    enhance();
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Copied"));
    expect(writeText).toHaveBeenCalledWith("<hello>\nworld");
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Copy failed"));
  });
  it("respects reduced motion and never hides content before observing it", () => {
    const observe = vi.fn();
    const construct = vi.fn();
    class Observer {
      constructor() { construct(); }
      observe = observe;
      unobserve = vi.fn();
    }
    vi.stubGlobal("IntersectionObserver", Observer);
    vi.stubGlobal("matchMedia", () => ({ matches: true }));
    document.body.innerHTML = '<section class="vy-motion-rise">Visible content</section>';
    enhance();
    expect(construct).not.toHaveBeenCalled();
    expect(screen.getByText("Visible content")).toBeVisible();
    vi.stubGlobal("matchMedia", () => ({ matches: false }));
    enhance();
    expect(observe).toHaveBeenCalledWith(screen.getByText("Visible content"));
    expect(screen.getByText("Visible content")).toBeVisible();
  });

});
