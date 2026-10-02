import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { CAPABILITIES } from "@/lib/capabilities";

/**
 * Groups `docs/ROUTE-ACCESS.md`'s routes by the capability that guards
 * them, into top-level `/api/v1` areas (`/audience/submissions` → the
 * `audience` area). `public`, `authenticated` and the one plugin-decided
 * route need no capability and are skipped; an `X or Y` row credits the
 * area to both `X` and `Y`, since holding either reaches it.
 */
function areasByCapability(doc: string): Map<string, Set<string>> {
  const areas = new Map<string, Set<string>>();
  const rowRe = /^\|\s*([A-Z]+)\s*\|\s*(\S+)\s*\|\s*(.+?)\s*\|$/gm;
  let m: RegExpExecArray | null;
  while ((m = rowRe.exec(doc)) !== null) {
    const [, , routePath, access] = m;
    if (access === undefined || routePath === undefined) continue;
    if (access === "public" || access === "authenticated" || access.includes("plugin-decided")) continue;
    const area = routePath.replace(/^\//, "").split("/")[0] ?? "";
    if (area === "") continue;
    for (const cap of access.split(" or ").map((c) => c.trim())) {
      if (!areas.has(cap)) areas.set(cap, new Set());
      areas.get(cap)?.add(area);
    }
  }
  return areas;
}

describe("capability descriptions stay in step with docs/ROUTE-ACCESS.md", () => {
  it("declares every area a capability's routes fall under, in capabilities.ts's own areas list", () => {
    const doc = readFileSync(resolve(process.cwd(), "../docs/ROUTE-ACCESS.md"), "utf8");
    const docAreas = areasByCapability(doc);
    expect(docAreas.size).toBeGreaterThan(5); // the parse actually found rows

    for (const [cap, areas] of docAreas) {
      const declared = new Set(CAPABILITIES[cap]?.areas ?? []);
      const missing = [...areas].filter((a) => !declared.has(a));
      expect(
        missing,
        `${cap} now guards route(s) under area(s) not listed in its \`areas\` in ` +
          `admin/src/lib/capabilities.ts: ${missing.join(", ")}. Review \`description\`/\`also\` ` +
          `for ${cap}, then add the area(s) so this test reflects the review.`,
      ).toEqual([]);
    }
  });
});
