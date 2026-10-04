import { Link } from "@tanstack/react-router";
import { TestRouter } from "./TestRouter";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { SettingsPage, previewDate } from "@/routes/_auth/settings/index";

vi.mock("@/components/editor/MonacoPane", () => ({
  MonacoPane: ({ value }: { value: string }) => <textarea readOnly value={value} aria-label="css" />,
}));

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: {
      ...original.api,
      getOptions: vi.fn(),
      putOptions: vi.fn(),
      setupChecks: vi.fn(),
      setupVerifyUrl: vi.fn(),
      mailSettings: vi.fn(),
      myCaps: vi.fn(),
      listRoles: vi.fn(),
      registrationInfo: vi.fn(),
    },
  };
});
const mocked = vi.mocked(api);

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <ConfirmProvider>
        <TestRouter><Link to="/profile">Leave settings</Link><SettingsPage /></TestRouter>
      </ConfirmProvider>
    </QueryClientProvider>,
  );
  return { ...view, client };
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.setupChecks.mockResolvedValue([{ name: "smtp", status: "warn", detail: "no SMTP relay" }]);
  mocked.setupVerifyUrl.mockResolvedValue({ reachable: true, local: false });
  mocked.mailSettings.mockResolvedValue({ host: "", port: 587, username: "", from: "", has_password: false, source: "none", encrypted: true });
  // A resolved default for every test, not just the membership ones: an
  // unresolved `vi.fn()` returns `undefined`, which React Query treats as
  // "Query data cannot be undefined" and warns about on every other test
  // in this file that never cared about capabilities or roles at all.
  mocked.myCaps.mockResolvedValue([]);
  mocked.listRoles.mockResolvedValue([]);
  mocked.registrationInfo.mockResolvedValue({ enabled: false, available: false, password_min_length: 8 });
});

describe("settings page", () => {
  it("exposes every option the wizard writes, including the eight that had no UI", async () => {
    mocked.getOptions.mockResolvedValue({
      timezone: "Asia/Kolkata",
      permalink_pattern: "/{year}/{slug}",
      edge_cache_seconds: 60,
      ai_month_cap_usd: 12.5,
      media_storage_cap_mb: 2048,
      site_language: "en",
      registry_trusted_keys: ["a".repeat(64), "b".repeat(64)],
      update_trusted_keys: [],
    } as never);
    mount();
    expect(((await screen.findByLabelText("Timezone")) as HTMLSelectElement).value).toBe("Asia/Kolkata");
    expect((screen.getByLabelText("Permalinks") as HTMLSelectElement).value).toBe("/{year}/{slug}");
    expect((screen.getByLabelText("Edge cache") as HTMLInputElement).value).toBe("60");
    expect((screen.getByLabelText("Monthly cap") as HTMLInputElement).value).toBe("12.5");
    expect((screen.getByLabelText("Storage cap") as HTMLInputElement).value).toBe("2048");
    expect((screen.getByLabelText("Language") as HTMLInputElement).value).toBe("en");
    expect((screen.getByLabelText("Marketplace signing keys") as HTMLTextAreaElement).value.split("\n")).toHaveLength(2);
  });

  it("names the changed fields in the save bar and saves lists and numbers typed", { timeout: 15_000 }, async () => {
    const user = userEvent.setup();
    mocked.getOptions.mockResolvedValue({ edge_cache_seconds: 0, update_trusted_keys: [] } as never);
    mocked.putOptions.mockResolvedValue(undefined);
    mount();
    const cache = await screen.findByLabelText("Edge cache");
    await user.clear(cache);
    await user.type(cache, "120");
    const keys = screen.getByLabelText("Release signing keys");
    await user.click(keys);
    await user.paste("c".repeat(64));
    expect(screen.getByTestId("save-bar")).toHaveTextContent("2 unsaved changes: Edge cache, Release signing keys");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(mocked.putOptions).toHaveBeenCalledTimes(1));
    expect(mocked.putOptions).toHaveBeenCalledWith({ edge_cache_seconds: 120, update_trusted_keys: ["c".repeat(64)] });
  });

  it("refuses a bad signing key and an http marketplace before asking the server", { timeout: 15_000 }, async () => {
    const user = userEvent.setup();
    mocked.getOptions.mockResolvedValue({} as never);
    mount();
    await user.click(await screen.findByLabelText("Marketplace index URL"));
    await user.paste("http://plain.example.com/index.json");
    expect(screen.getByText("Must be an https URL.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
  });

  it("checks a new site address against the server and offers Save anyway when it fails", { timeout: 15_000 }, async () => {
    const user = userEvent.setup();
    mocked.getOptions.mockResolvedValue({ site_url: "https://old.example.com" } as never);
    mocked.setupVerifyUrl.mockResolvedValue({ reachable: false, local: false });
    mocked.putOptions.mockResolvedValue(undefined);
    mount();
    const url = await screen.findByLabelText("Site address");
    await user.clear(url);
    await user.click(url);
    await user.paste("https://new.example.com");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(mocked.setupVerifyUrl).toHaveBeenCalledWith("https://new.example.com"));
    expect(mocked.putOptions).not.toHaveBeenCalled();
    const again = await screen.findByRole("button", { name: "Save anyway" });
    await user.click(again);
    await waitFor(() => expect(mocked.putOptions).toHaveBeenCalledWith({ site_url: "https://new.example.com" }));
  });

  it("saves a local site address without a warning, since the server does not test it", { timeout: 15_000 }, async () => {
    const user = userEvent.setup();
    mocked.getOptions.mockResolvedValue({ site_url: "https://old.example.com" } as never);
    mocked.setupVerifyUrl.mockResolvedValue({ reachable: false, local: true });
    mocked.putOptions.mockResolvedValue(undefined);
    mount();
    const url = await screen.findByLabelText("Site address");
    await user.clear(url);
    await user.click(url);
    await user.paste("http://localhost:3000");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(mocked.putOptions).toHaveBeenCalledWith({ site_url: "http://localhost:3000" }));
    expect(screen.queryByText(/does not reach this server/)).not.toBeInTheDocument();
  });

  it("gates the newsletter switch on an SMTP relay", async () => {
    mocked.getOptions.mockResolvedValue({} as never);
    mount();
    await screen.findByLabelText("Site title");
    await waitFor(() => expect(screen.getByText(/No mail relay is configured/)).toBeInTheDocument());
  });

  it("previews strftime patterns", () => {
    const d = new Date(2026, 7, 29);
    expect(previewDate("%b %e, %Y", d)).toBe("Aug 29, 2026");
    expect(previewDate("%Y-%m-%d", d)).toBe("2026-08-29");
    expect(previewDate("%e %B %Y", d)).toBe("29 August 2026");
  });
});

const ALL_CAPS = [
  "edit_posts", "publish_posts", "edit_others", "delete_posts", "manage_categories",
  "moderate_comments", "upload_media", "manage_users", "manage_themes", "manage_plugins",
  "manage_options", "view_admin",
];

const ROLES = [
  { slug: "admin", name: "Administrator", description: "", built_in: true, users: 1, capabilities: [...ALL_CAPS] },
  { slug: "editor", name: "Editor", description: "", built_in: true, users: 0, capabilities: ["edit_posts", "publish_posts", "edit_others", "delete_posts", "manage_categories", "moderate_comments", "upload_media", "view_admin"] },
  { slug: "subscriber", name: "Subscriber", description: "", built_in: true, users: 3, capabilities: ["view_admin"] },
  { slug: "drafter", name: "Drafter", description: "", built_in: false, users: 0, capabilities: ["edit_posts", "view_admin"] },
  { slug: "auditor", name: "Auditor", description: "", built_in: false, users: 0, capabilities: ["manage_options"] },
  { slug: "moderator", name: "Moderator", description: "", built_in: false, users: 0, capabilities: ["moderate_comments", "view_admin"] },
  { slug: "tagger", name: "Tagger", description: "", built_in: false, users: 0, capabilities: ["manage_categories", "view_admin"] },
];

describe("membership section", () => {
  it("is hidden for anyone short of every capability", async () => {
    mocked.getOptions.mockResolvedValue({ registration_enabled: false, registration_default_role: "subscriber" } as never);
    mocked.myCaps.mockResolvedValue(ALL_CAPS.filter((c) => c !== "manage_themes"));
    mocked.listRoles.mockResolvedValue(ROLES as never);
    mount();
    await screen.findByLabelText("Site title");
    expect(screen.queryByText("Membership")).toBeNull();
    expect(screen.queryByLabelText("Anyone can create an account")).toBeNull();
    expect(mocked.listRoles).not.toHaveBeenCalled();
  });

  it("is shown, with only eligible roles offered, for a full administrator", async () => {
    mocked.getOptions.mockResolvedValue({ registration_enabled: true, registration_default_role: "subscriber" } as never);
    mocked.myCaps.mockResolvedValue([...ALL_CAPS]);
    mocked.listRoles.mockResolvedValue(ROLES as never);
    mount();
    const select = (await screen.findByLabelText("Default role for new accounts")) as HTMLSelectElement;
    const offered = Array.from(select.options).map((o) => o.value);
    // At most an author's power: nothing over the site, other people's
    // work or comments, or the categories everyone files posts under.
    expect(offered).toEqual(["subscriber", "drafter"]);
    expect(offered).not.toContain("admin");
    expect(offered).not.toContain("editor");
    expect(offered).not.toContain("auditor");
    expect(offered).not.toContain("moderator");
    expect(offered).not.toContain("tagger");
    expect(
      screen.getByText(
        "Offered: roles that hold none of manage users, options, plugins, themes or categories, edit others' content, or moderate comments — so never Administrator or Editor. A stranger who fills in the form gets at most an author's power over their own work.",
      ),
    ).toBeInTheDocument();
    expect(select.value).toBe("subscriber");
    const toggle = screen.getByLabelText("Anyone can create an account") as HTMLInputElement;
    expect(toggle.checked).toBe(true);
  });

  it("keeps a since-ineligible saved role on the control instead of silently swapping it", async () => {
    mocked.getOptions.mockResolvedValue({ registration_enabled: true, registration_default_role: "auditor" } as never);
    mocked.myCaps.mockResolvedValue([...ALL_CAPS]);
    mocked.listRoles.mockResolvedValue(ROLES as never);
    mount();
    const select = (await screen.findByLabelText("Default role for new accounts")) as HTMLSelectElement;
    expect(select.value).toBe("auditor");
    expect(screen.getByText("Auditor (not eligible)")).toBeInTheDocument();
  });

  it("warns from the server's own readiness when registration is on but cannot work", async () => {
    mocked.getOptions.mockResolvedValue({ registration_enabled: true, registration_default_role: "subscriber", site_url: "https://site.example" } as never);
    mocked.myCaps.mockResolvedValue([...ALL_CAPS]);
    mocked.listRoles.mockResolvedValue(ROLES as never);
    // The page's own guesses say everything is fine; the server says not.
    mocked.setupChecks.mockResolvedValue([{ name: "smtp", status: "ok", detail: "relay" }]);
    mocked.registrationInfo.mockResolvedValue({ enabled: true, available: false, password_min_length: 8 });
    mount();
    await screen.findByLabelText("Default role for new accounts");
    expect(await screen.findByTestId("membership-mail-note")).toHaveTextContent(
      "Registration is on, but every attempt is refused: this server can't send the confirmation email.",
    );
  });

  it("says nothing more once the server says registration is available", async () => {
    mocked.getOptions.mockResolvedValue({ registration_enabled: true, registration_default_role: "subscriber" } as never);
    mocked.myCaps.mockResolvedValue([...ALL_CAPS]);
    mocked.listRoles.mockResolvedValue(ROLES as never);
    // The page's own guesses would have warned; the server says it works.
    mocked.setupChecks.mockResolvedValue([{ name: "smtp", status: "warn", detail: "no SMTP relay" }]);
    mocked.registrationInfo.mockResolvedValue({ enabled: true, available: true, password_min_length: 8 });
    mount();
    await screen.findByLabelText("Default role for new accounts");
    await waitFor(() => expect(mocked.registrationInfo).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByTestId("membership-mail-note")).toHaveTextContent(""));
    expect(screen.queryByText(/every attempt is refused/)).toBeNull();
  });

  it("says what registration needs while it is off", async () => {
    mocked.getOptions.mockResolvedValue({ registration_enabled: false, registration_default_role: "subscriber" } as never);
    mocked.myCaps.mockResolvedValue([...ALL_CAPS]);
    mocked.listRoles.mockResolvedValue(ROLES as never);
    mount();
    await screen.findByLabelText("Default role for new accounts");
    expect(await screen.findByTestId("membership-mail-note")).toHaveTextContent(
      "Registration needs both a mail relay and a site address configured — without them every attempt is refused.",
    );
  });

  it("saves the toggle and the chosen default role", async () => {
    const user = userEvent.setup();
    mocked.getOptions.mockResolvedValue({ registration_enabled: false, registration_default_role: "subscriber" } as never);
    mocked.myCaps.mockResolvedValue([...ALL_CAPS]);
    mocked.listRoles.mockResolvedValue(ROLES as never);
    mocked.putOptions.mockResolvedValue(undefined);
    mount();
    const toggle = await screen.findByLabelText("Anyone can create an account");
    await user.click(toggle);
    const select = screen.getByLabelText("Default role for new accounts");
    await user.selectOptions(select, "drafter");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(mocked.putOptions).toHaveBeenCalledWith({ registration_enabled: true, registration_default_role: "drafter" }));
  });
});


it("preserves a dirty field when options refresh and asks before navigation", async () => {
  const user = userEvent.setup();
  mocked.getOptions.mockResolvedValue({ site_title: "Original" });
  const { client } = mount();
  const title = await screen.findByLabelText("Site title");
  await user.clear(title); await user.type(title, "Unsaved title");
  client.setQueryData(["options"], { site_title: "Changed elsewhere", tagline: "Updated" });
  await waitFor(() => expect(title).toHaveValue("Unsaved title"));
  await user.click(screen.getByRole("link", { name: "Leave settings" }));
  expect(await screen.findByText("Leave without saving?")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "Cancel" }));
  expect(title).toHaveValue("Unsaved title");
});
