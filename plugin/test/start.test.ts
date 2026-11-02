// farol.start: a board's new session in the chosen workspace with the chosen
// agent profile; and the boards' language.
import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS } from "../shared/settings";
import { boardLanguage, type CreateInput, DEVICE_LABEL, type StartPorts, startSession } from "../server/start";

function ports(patch: Partial<StartPorts> = {}, settings: Partial<typeof DEFAULT_SETTINGS> = {}) {
  const created: Array<{ workspaceId: string; input: CreateInput }> = [];
  const p: StartPorts = {
    settings: () => ({ ...DEFAULT_SETTINGS, workspaceId: "ws-1", profileId: "fast", ...settings }),
    profile: async (id) => (id === "fast" ? { id, provider: "claude", modeId: "bypass" } : null),
    defaultModel: async (provider) => (provider === "claude" ? "opus-5.5" : null),
    create: async (workspaceId, input) => {
      created.push({ workspaceId, input });
      return { id: "agent-9" };
    },
    log: { warn: () => {} },
    ...patch,
  };
  return { p, created };
}

const start = { device: "farol-3a1f", text: "Make the login error friendlier" };

describe("starting a session from a board", () => {
  it("creates the agent in the workspace with the profile, the text as its first prompt", async () => {
    const { p, created } = ports();
    expect(await startSession(p, start)).toEqual({ ok: true, agent: "agent-9" });
    expect(created).toEqual([
      {
        workspaceId: "ws-1",
        input: {
          config: { provider: "claude/opus-5.5", modeId: "bypass" },
          prompt: start.text,
          labels: { [DEVICE_LABEL]: "farol-3a1f" },
        },
      },
    ]);
  });

  it("refuses without a workspace or profile, and on read-only boards", async () => {
    expect(await startSession(ports({}, { workspaceId: "" }).p, start)).toMatchObject({ reason: "no_workspace" });
    expect(await startSession(ports({}, { profileId: "gone" }).p, start)).toMatchObject({ reason: "no_workspace" });
    expect(await startSession(ports({}, { allowActions: false }).p, start)).toMatchObject({ reason: "read_only" });
  });

  it("reports Paseo's refusal instead of throwing", async () => {
    const { p } = ports({
      create: async () => {
        throw new Error("workspace has no directory");
      },
    });
    expect(await startSession(p, start)).toEqual({
      ok: false,
      reason: "failed",
      message: "workspace has no directory",
    });
  });
});

describe("the boards' language", () => {
  it("follows the Paseo app unless set, and stays unknown until the app reports", () => {
    expect(boardLanguage("auto", null)).toBeUndefined();
    expect(boardLanguage("auto", "zh-CN")).toBe("zh");
    expect(boardLanguage("auto", "en")).toBe("en");
    expect(boardLanguage("zh", "en")).toBe("zh");
  });
});
