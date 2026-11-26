// The docs plugin's script (plugins/docs/assets/docs.js) is served on every page of a site. Its code-block copy
// buttons and table wrapping must touch the docs page body only, never a theme's own code panels or tables
// elsewhere (a home page's `pre.code` got a second Copy button and the theme's room for it).
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const script = readFileSync(resolve(process.cwd(), "../plugins/docs/assets/docs.js"), "utf8");

function run(html: string) {
  document.body.innerHTML = html;
  new Function(script)();
}

describe("docs plugin script scope", () => {
  beforeEach(() => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: () => Promise.resolve() }, configurable: true });
  });
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("leaves a non-docs page's code blocks and tables untouched", () => {
    const page =
      '<div class="vy-section" id="agents" data-section="agents"><div class="kp"><div class="card">' +
      '<pre class="code"><code>{"tool": "sql", "args": {}}</code></pre>' +
      "<table><tbody><tr><td>Streaming</td><td>Supported</td></tr></tbody></table>" +
      "</div></div></div>";
    run(page);
    expect(document.body.innerHTML).toBe(page);
    expect(document.querySelector(".docs-copy")).toBeNull();
    expect((document.querySelector("pre.code") as HTMLElement).getAttribute("style")).toBeNull();
  });

  it("adds a copy button and wraps tables inside the docs body only", () => {
    run(
      '<div data-section="hero"><pre class="code"><code>home</code></pre><table><tbody><tr><td>no</td></tr></tbody></table></div>' +
        '<div data-section="docs-layout"><div data-section="docs-main">' +
        '<div data-section="docs-content"><pre class="vy-code"><code>SELECT 1</code></pre>' +
        "<table><tbody><tr><td>Window</td><td>Supported</td></tr></tbody></table></div>" +
        '<div data-section="docs-pager"></div></div></div>',
    );
    const buttons = document.querySelectorAll(".docs-copy");
    expect(buttons).toHaveLength(1);
    expect(buttons.item(0).closest('[data-section="docs-content"]')).not.toBeNull();
    expect(document.querySelector('[data-section="hero"] pre')?.querySelector(".docs-copy")).toBeNull();
    expect(document.querySelectorAll(".docs-table")).toHaveLength(1);
    expect(document.querySelector('[data-section="hero"] .docs-table')).toBeNull();
    expect(document.querySelector('[data-section="hero"] .docs-pill')).toBeNull();
    expect(document.querySelector('[data-section="docs-content"] .docs-pill--ok')?.textContent).toBe("Supported");
  });

  it("treats a container a theme marks with data-docs-copy like the docs body", () => {
    run(
      '<div class="kc-read" data-docs-copy><pre><code>pip install krishiv</code></pre></div>' +
        '<div class="kp"><pre class="code"><code>home</code></pre></div>',
    );
    expect(document.querySelectorAll(".docs-copy")).toHaveLength(1);
    expect(document.querySelector(".kc-read pre .docs-copy")).not.toBeNull();
    expect(document.querySelector(".kp pre .docs-copy")).toBeNull();
  });
});
