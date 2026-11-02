// Model labels: the user's profile names, then the provider catalog, then a cleaned id.
import { describe, expect, it, vi } from "vitest";
import { type ModelDirectory, MODEL_NAMES_RETRY_MS, MODEL_NAMES_TTL_MS, ModelNames, modelLabelFor, prettyModel } from "../server/models";

// Profiles as the user's daemon has them.
const directory: ModelDirectory = {
  profiles: [
    { provider: "claude", model: "claude-sonnet-5-5", name: "Sonnet 5.5" },
    { provider: "antigravity", model: "claude-opus-5-5-high", name: "Opus 5.5 (Agy)" },
    { provider: "opencode", model: "zhipuai-coding-plan/glm-5.3", name: "GLM 5.3" },
    { provider: "codex", model: "gpt-6.1-sol", name: "GPT 6.1 Sol" },
  ],
  catalogs: {
    opencode: [{ id: "kimi-code-plan-cn/k3.1-pro", label: "Kimi K3.1 Pro" }],
    claude: [{ id: "claude-opus-5-5", label: "Opus 5.5 catalog", aliases: ["opus"] }],
  },
};

describe("model labels", () => {
  it("a profile's name wins for the same provider and model", () => {
    expect(modelLabelFor(directory, "claude", [null, "claude-sonnet-5-5"])).toBe("Sonnet 5.5");
    expect(modelLabelFor(directory, "antigravity", ["claude-opus-5-5-high"])).toBe("Opus 5.5 (Agy)");
    expect(modelLabelFor(directory, "opencode", [undefined, "zhipuai-coding-plan/glm-5.3"])).toBe("GLM 5.3");
    expect(modelLabelFor(directory, "codex", ["gpt-6.1-sol", "gpt-6.1-sol"])).toBe("GPT 6.1 Sol");
    // The runtime model is tried before the configured one.
    expect(modelLabelFor(directory, "codex", ["gpt-6.1-sol", "gpt-6.1-terra"])).toBe("GPT 6.1 Sol");
    // Same model, other provider: not that profile.
    expect(modelLabelFor(directory, "claude", ["gpt-6.1-sol"])).toBe("GPT-6.1 Sol");
  });

  it("then the provider catalog's label (by id or alias), then a cleaned id", () => {
    expect(modelLabelFor(directory, "opencode", [null, "kimi-code-plan-cn/k3.1-pro"])).toBe("Kimi K3.1 Pro");
    expect(modelLabelFor(directory, "claude", ["opus"])).toBe("Opus 5.5 catalog");
    expect(modelLabelFor(undefined, "opencode", ["kimi-code-plan-cn/k3.1-pro"])).toBe("K3.1 Pro");
    expect(modelLabelFor(directory, "claude", [null, null])).toBe("");
  });

  it("cleaned ids are properly cased", () => {
    expect(prettyModel("claude-opus-5-5")).toBe("Opus 5.5");
    expect(prettyModel("claude-sonnet-5-5-20261001")).toBe("Sonnet 5.5");
    expect(prettyModel("gpt-6.1-sol")).toBe("GPT-6.1 Sol");
    expect(prettyModel("codex/gpt-6.1-terra")).toBe("GPT-6.1 Terra");
    expect(prettyModel("zhipuai-coding-plan/glm-5.3")).toBe("GLM-5.3");
    expect(prettyModel("claude-opus-5-5[1m]")).toBe("Opus 5.5");
    expect(prettyModel("deepseek-v4")).toBe("DeepSeek V4");
    expect(prettyModel(null)).toBe("");
    expect(prettyModel("some-really-long-model-name-for-testing-purposes")).toBe("Some Really Long Model…");
  });
});

describe("model names cache", () => {
  function fake() {
    let now = 1_000_000;
    const get = vi.fn(async () => ({
      requestId: "r",
      config: { agentProfiles: [{ id: "p", name: "Sonnet 5.5", provider: "claude", model: "claude-sonnet-5-5" }] },
    }));
    const listModels = vi.fn(async (provider: string) => ({
      provider,
      models: [{ provider, id: "claude-opus-5-5", label: "Opus 5.5" }],
      fetchedAt: "",
      requestId: "r",
    }));
    const onChange = vi.fn();
    const names = new ModelNames(
      { config: { get }, providers: { listModels } } as never,
      onChange,
      { warn() {} },
      () => now,
    );
    return { names, get, listModels, onChange, advance: (ms: number) => (now += ms) };
  }

  it("answers from the cache at once, fills it in the background, refreshes after the TTL", async () => {
    const { names, get, listModels, onChange, advance } = fake();
    expect(names.directory(["claude", "claude"])).toEqual({ profiles: [], catalogs: { claude: [] } });
    names.directory(["claude"]);
    expect(get).toHaveBeenCalledTimes(1);
    expect(listModels).toHaveBeenCalledTimes(1);
    await vi.waitFor(() => expect(onChange).toHaveBeenCalledTimes(2));
    const dir = names.directory(["claude"]);
    expect(modelLabelFor(dir, "claude", ["claude-sonnet-5-5"])).toBe("Sonnet 5.5");
    expect(modelLabelFor(dir, "claude", ["claude-opus-5-5"])).toBe("Opus 5.5");
    expect(get).toHaveBeenCalledTimes(1);
    advance(MODEL_NAMES_TTL_MS);
    names.directory(["claude"]);
    expect(get).toHaveBeenCalledTimes(2);
    expect(listModels).toHaveBeenCalledTimes(2);
    // Same answers: no extra change.
    await new Promise((r) => setTimeout(r, 0));
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it("a failed lookup keeps the old names and retries sooner", async () => {
    const { names, get, advance } = fake();
    get.mockRejectedValueOnce(new Error("down"));
    names.directory([]);
    await new Promise((r) => setTimeout(r, 0));
    names.directory([]);
    expect(get).toHaveBeenCalledTimes(1);
    advance(MODEL_NAMES_RETRY_MS);
    names.directory([]);
    expect(get).toHaveBeenCalledTimes(2);
  });
});
