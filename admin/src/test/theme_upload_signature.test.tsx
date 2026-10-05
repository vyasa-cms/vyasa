import { describe, expect, it, vi } from "vitest";
import { installTheme } from "@/api/themes";

describe("theme upload", () => {
  it("sends the signature beside the file when one is given", async () => {
    const calls: FormData[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_url: string, init: RequestInit) => {
      calls.push(init.body as FormData);
      return new Response(JSON.stringify({ id: 1, name: "t", version: 1, is_active: false, package_version: 1 }), { status: 201, headers: { "content-type": "application/json" } });
    }));
    const file = new File([new Uint8Array([1, 2, 3])], "t.vytheme");
    await installTheme(file, "abcd");
    expect(calls[0]?.get("signature")).toBe("abcd");
    await installTheme(file);
    expect(calls[1]?.has("signature")).toBe(false);
    vi.unstubAllGlobals();
  });
});
