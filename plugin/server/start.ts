// `farol.start`: a new agent session from a board, in the workspace and with
// the agent profile chosen in the plugin's settings. The text (typed or
// dictated on the board) is the first prompt. Paseo derives the title.
import type { ActResult, Start } from "../shared/protocol";
import type { Settings } from "../shared/settings";

/** The parts of a Paseo agent profile a session needs. */
export interface ProfileFacts {
  id: string;
  name?: string;
  provider: string;
  model?: string;
  modeId?: string;
  thinkingOptionId?: string;
  featureValues?: Record<string, unknown>;
}

export interface CreateInput {
  config: {
    provider: string;
    modeId?: string;
    thinkingOptionId?: string;
    featureValues?: Record<string, unknown>;
  };
  prompt: string;
  labels: Record<string, string>;
}

export interface StartPorts {
  settings(): Pick<Settings, "allowActions" | "workspaceId" | "profileId">;
  profile(id: string): Promise<ProfileFacts | null>;
  /** The provider's default model, for a profile that names none. */
  defaultModel(provider: string): Promise<string | null>;
  create(workspaceId: string, input: CreateInput): Promise<{ id: string }>;
  log: { warn(...a: unknown[]): void };
}

/** Label on agents a board started: the board's id. */
export const DEVICE_LABEL = "farol.device";

export async function startSession(ports: StartPorts, start: Start): Promise<ActResult> {
  const settings = ports.settings();
  if (!settings.allowActions) return { ok: false, reason: "read_only" };
  if (!settings.workspaceId || !settings.profileId) return { ok: false, reason: "no_workspace" };
  try {
    const profile = await ports.profile(settings.profileId);
    if (!profile) {
      return { ok: false, reason: "no_workspace", message: "the agent profile is gone" };
    }
    const model = profile.model ?? (await ports.defaultModel(profile.provider));
    if (!model) return { ok: false, reason: "failed", message: `no model for ${profile.provider}` };
    const agent = await ports.create(settings.workspaceId, {
      config: {
        provider: `${profile.provider}/${model}`,
        ...(profile.modeId ? { modeId: profile.modeId } : {}),
        ...(profile.thinkingOptionId ? { thinkingOptionId: profile.thinkingOptionId } : {}),
        ...(profile.featureValues ? { featureValues: profile.featureValues } : {}),
      },
      prompt: start.text,
      labels: { [DEVICE_LABEL]: start.device },
    });
    return { ok: true, agent: agent.id };
  } catch (error) {
    ports.log.warn("farol: start failed", String(error));
    return { ok: false, reason: "failed", message: String(error instanceof Error ? error.message : error) };
  }
}

/** The boards' language: the setting, else the Paseo app's (any Chinese locale is "zh"). */
export function boardLanguage(
  setting: Settings["boardLanguage"],
  appLocale: string | null,
): "zh" | "en" | undefined {
  if (setting !== "auto") return setting;
  if (!appLocale) return undefined;
  return appLocale.toLowerCase().startsWith("zh") ? "zh" : "en";
}
