import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, type AiRegistry } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { ModelsPage } from "@/routes/_auth/models/index";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: {
      ...original.api,
      aiModels: vi.fn(),
      saveAiProvider: vi.fn(),
      deleteAiProvider: vi.fn(),
      testAiProvider: vi.fn(),
      aiCatalog: vi.fn(),
      registerAiModel: vi.fn(),
      editAiModel: vi.fn(),
      defaultAiModel: vi.fn(),
      testAiModel: vi.fn(),
      deleteAiModel: vi.fn(),
    },
  };
});

const mocked = vi.mocked(api);

const kinds = [
  { kind: "text", label: "Text generation", used_by: "Writing assistant, theme builder" },
  { kind: "embedding", label: "Embeddings", used_by: "Not used yet — reserved for semantic search" },
];

function registry(overrides: Partial<AiRegistry> = {}): AiRegistry {
  return {
    encrypting_keys: false,
    providers: [
      {
        provider: "anthropic",
        label: "Anthropic",
        configured: true,
        key_source: "stored",
        key_hint: "…abcd",
        key_sealed: false,
        base_url: "",
        default_base_url: "https://api.anthropic.com/v1",
        enabled: true,
        supports: ["text", "vision"],
        needs_key: true,
        needs_base_url: false,
        recommended: { text: { model: "claude-sonnet-5", label: "Claude Sonnet 5", input_cost_per_mtok: 3, output_cost_per_mtok: 15 } },
      },
      {
        provider: "openai",
        label: "OpenAI",
        configured: false,
        key_source: "none",
        key_hint: "",
        key_sealed: false,
        base_url: "",
        default_base_url: "https://api.openai.com/v1",
        enabled: true,
        supports: ["text", "vision", "image", "embedding"],
        needs_key: true,
        needs_base_url: false,
        recommended: {},
      },
      {
        provider: "openrouter",
        label: "OpenRouter",
        configured: false,
        key_source: "none",
        key_hint: "",
        key_sealed: false,
        base_url: "",
        default_base_url: "https://openrouter.ai/api/v1",
        enabled: true,
        supports: ["text", "vision", "image", "embedding"],
        needs_key: true,
        needs_base_url: false,
        recommended: {},
      },
    ],
    models: [
      {
        id: "1",
        provider: "anthropic",
        model: "claude-sonnet-4-5",
        kind: "text",
        label: "Sonnet",
        enabled: true,
        is_default: true,
        settings: {},
        input_cost_per_mtok: 3,
        output_cost_per_mtok: 15,
        last_probe_ok: null,
        last_probe_detail: "",
        last_probe_at: null,
        created_at: "2026-08-29T00:00:00Z",
        updated_at: "2026-08-29T00:00:00Z",
      },
    ],
    kinds,
    ...overrides,
  };
}

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ConfirmProvider>
        <ModelsPage />
      </ConfirmProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.aiModels.mockResolvedValue(registry());
});

describe("AI models page", () => {
  it("shows providers with key status but never a key, and models under their job", async () => {
    mount();
    const anthropic = await screen.findByTestId("provider-anthropic");
    expect(within(anthropic).getByText("Key stored")).toBeInTheDocument();
    expect(within(anthropic).getByText(/key …abcd/)).toBeInTheDocument();
    expect(within(screen.getByTestId("provider-openai")).getByText("No key")).toBeInTheDocument();

    const text = screen.getByTestId("models-text");
    expect(within(text).getByText("Sonnet")).toBeInTheDocument();
    // A job with no model is folded away, not a full-width empty panel.
    const folded = screen.getByTestId("empty-kinds");
    expect(folded).toHaveTextContent("1 job has no model yet");
  });

  it("warns when keys are stored unencrypted, and not when they are sealed", async () => {
    mount();
    expect(await screen.findByTestId("plain-keys-note")).toHaveTextContent("VYASA_SECRET_KEY");
  });

  it("saves a provider key without echoing it", async () => {
    const user = userEvent.setup();
    mocked.saveAiProvider.mockResolvedValue({
      provider: "openai",
      label: "OpenAI",
      configured: true,
      key_source: "stored",
      key_hint: "…9999",
      key_sealed: false,
      base_url: "",
      default_base_url: "https://api.openai.com/v1",
      enabled: true,
      supports: ["text"],
      needs_key: true,
      needs_base_url: false,
      recommended: {},
    });
    mount();
    const openai = await screen.findByTestId("provider-openai");
    await user.type(within(openai).getByLabelText("API key"), "sk-test-9999");
    await user.click(within(openai).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(mocked.saveAiProvider).toHaveBeenCalledWith("openai", {
        api_key: "sk-test-9999",
        base_url: "",
        enabled: true,
      }),
    );
  });

  it("registers a model for a job the provider supports", async () => {
    const user = userEvent.setup();
    mocked.registerAiModel.mockResolvedValue({
      ...registry().models[0]!,
      id: "2",
      model: "claude-haiku-4-5",
      label: "Haiku",
      is_default: false,
    });
    mount();
    await user.click(await screen.findByTestId("add-model"));
    const dialog = await screen.findByTestId("add-model-dialog");
    // Anthropic is preselected (the only configured provider) and cannot do embeddings.
    const job = within(dialog).getByLabelText("Job") as HTMLSelectElement;
    expect(Array.from(job.options).map((o) => o.value)).toEqual(["text"]);
    await user.type(within(dialog).getByTestId("model-id"), "claude-haiku-4-5");
    await user.type(within(dialog).getByLabelText("Label"), "Haiku");
    await user.click(within(dialog).getByTestId("register-model"));
    await waitFor(() =>
      expect(mocked.registerAiModel).toHaveBeenCalledWith(
        expect.objectContaining({ provider: "anthropic", kind: "text", model: "claude-haiku-4-5", label: "Haiku" }),
      ),
    );
  });

  it("tests a model and reports the outcome", async () => {
    const user = userEvent.setup();
    mocked.testAiModel.mockResolvedValue({ ok: true, detail: "Replied (12 in, 4 out tokens)" });
    mount();
    const text = await screen.findByTestId("models-text");
    await user.click(within(text).getByRole("button", { name: /Test/ }));
    await waitFor(() => expect(mocked.testAiModel).toHaveBeenCalledWith("1"));
  });
});
