import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";

const fetchMock = vi.fn();

afterEach(() => {
  vi.unstubAllGlobals();
  fetchMock.mockReset();
});

function sentBody(): unknown {
  const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
  return JSON.parse(init.body as string);
}

describe("POST /auth/verify body", () => {
  it("carries the password when one is given", async () => {
    vi.stubGlobal("fetch", fetchMock);
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await api.verifyEmail("tok", "my-password");
    expect(fetchMock.mock.calls[0]?.[0]).toMatch(/\/api\/v1\/auth\/verify$/);
    expect(sentBody()).toEqual({ token: "tok", password: "my-password" });
  });

  it("leaves the password out entirely when none is given", async () => {
    vi.stubGlobal("fetch", fetchMock);
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await api.verifyEmail("tok");
    expect(sentBody()).toEqual({ token: "tok" });
  });
});
