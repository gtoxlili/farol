// Model names for the card header. The user's own agent-profile names come
// first, then the provider catalog's label, then a cleaned-up id. Profiles and
// catalogs are cached here and refreshed in the background; a board poll only
// ever reads the cache. Owned by Farol; single-threaded (Node event loop).
import type { PaseoApi } from "./paseo-types";
import { clip } from "./text";

/** The board clips longer labels anyway. */
export const MODEL_LABEL_MAX = 24;
/** How long profiles and a provider's catalog stay fresh. */
export const MODEL_NAMES_TTL_MS = 10 * 60 * 1000;
/** After a failed fetch, try again this much sooner than a full TTL. */
export const MODEL_NAMES_RETRY_MS = 60 * 1000;

export interface ProfileName {
  provider: string;
  model?: string;
  name?: string;
}

export interface CatalogModel {
  id: string;
  label: string;
  aliases?: string[];
}

/** What the digest needs to name models: profiles, and catalogs by provider. */
export interface ModelDirectory {
  profiles: ProfileName[];
  catalogs: Record<string, CatalogModel[]>;
}

/** Acronyms and names with their own casing; any other word is capitalised. */
const SPECIAL_WORDS: Record<string, string> = {
  gpt: "GPT",
  glm: "GLM",
  oss: "OSS",
  qwq: "QwQ",
  deepseek: "DeepSeek",
  minimax: "MiniMax",
  llama: "Llama",
};
/** Acronyms that join their version with a hyphen ("GPT-6.1"). */
const HYPHEN_BEFORE_VERSION = new Set(["GPT", "GLM"]);

/** A readable label from a model id: "claude-opus-5-5" -> "Opus 5.5", "gpt-6.1-sol" -> "GPT-6.1 Sol". */
export function prettyModel(model: string | null | undefined): string {
  if (!model) return "";
  let m = model.trim();
  const slash = m.lastIndexOf("/");
  if (slash >= 0) m = m.slice(slash + 1);
  m = m
    .replace(/\[.*\]$/, "")
    .replace(/^claude-/i, "")
    .replace(/[-_](\d{8}|latest)$/i, "");
  // "5-5" -> "5.5" between version digits.
  m = m.replace(/(\d+)-(\d+)(?=$|[-_])/g, "$1.$2");
  const words = m.split(/[-_\s]+/).filter(Boolean);
  const out: string[] = [];
  for (const word of words) {
    const lower = word.toLowerCase();
    const cased =
      SPECIAL_WORDS[lower] ?? (/^[a-z]\d/i.test(word) ? word.toUpperCase() : word[0]!.toUpperCase() + word.slice(1));
    const prev = out[out.length - 1];
    if (prev && HYPHEN_BEFORE_VERSION.has(prev) && /^\d/.test(cased)) {
      out[out.length - 1] = `${prev}-${cased}`;
    } else {
      out.push(cased);
    }
  }
  return clip(out.join(" "), MODEL_LABEL_MAX);
}

/**
 * The label for an agent's model: a matching profile's name (same provider and
 * model), else the provider catalog's label, else `prettyModel`. `models` are
 * the ids to try, most specific first (runtime model, then configured model).
 */
export function modelLabelFor(
  directory: ModelDirectory | undefined,
  provider: string,
  models: ReadonlyArray<string | null | undefined>,
): string {
  const ids = [...new Set(models.filter((m): m is string => !!m))];
  if (!ids.length) return "";
  if (directory) {
    for (const id of ids) {
      const profile = directory.profiles.find((p) => p.provider === provider && p.model === id && p.name?.trim());
      if (profile) return clip(profile.name, MODEL_LABEL_MAX);
    }
    const catalog = directory.catalogs[provider] ?? [];
    for (const id of ids) {
      const entry = catalog.find((c) => c.id === id || c.aliases?.includes(id));
      if (entry?.label.trim()) return clip(entry.label, MODEL_LABEL_MAX);
    }
  }
  return prettyModel(ids[0]);
}

interface Slot<T> {
  value: T;
  /** When the value counts as stale, epoch ms. */
  freshUntil: number;
  loading: boolean;
}

type ModelPorts = Pick<PaseoApi, "config" | "providers">;

/**
 * Cache of profiles and provider catalogs. `directory()` answers from the
 * cache at once and starts a refresh for whatever is missing or stale;
 * `onChange` fires when new names arrive.
 */
export class ModelNames {
  private profiles: Slot<ProfileName[]> = { value: [], freshUntil: 0, loading: false };
  private readonly catalogs = new Map<string, Slot<CatalogModel[]>>();
  private closed = false;

  constructor(
    private readonly paseo: ModelPorts,
    private readonly onChange: () => void,
    private readonly log: { warn(...a: unknown[]): void } = console,
    private readonly now: () => number = Date.now,
  ) {}

  directory(providers: Iterable<string>): ModelDirectory {
    const now = this.now();
    if (!this.profiles.loading && now >= this.profiles.freshUntil) void this.loadProfiles();
    const catalogs: Record<string, CatalogModel[]> = {};
    for (const provider of new Set(providers)) {
      let slot = this.catalogs.get(provider);
      if (!slot) {
        slot = { value: [], freshUntil: 0, loading: false };
        this.catalogs.set(provider, slot);
      }
      if (!slot.loading && now >= slot.freshUntil) void this.loadCatalog(provider, slot);
      catalogs[provider] = slot.value;
    }
    return { profiles: this.profiles.value, catalogs };
  }

  close(): void {
    this.closed = true;
  }

  private async loadProfiles(): Promise<void> {
    const slot = this.profiles;
    slot.loading = true;
    try {
      const { config } = await this.paseo.config.get();
      const next = (config.agentProfiles ?? []).map((p) => ({ provider: p.provider, model: p.model, name: p.name }));
      slot.freshUntil = this.now() + MODEL_NAMES_TTL_MS;
      this.settle(slot, next);
    } catch (error) {
      slot.freshUntil = this.now() + MODEL_NAMES_RETRY_MS;
      this.log.warn("farol: agent profiles lookup failed", String(error));
    } finally {
      slot.loading = false;
    }
  }

  private async loadCatalog(provider: string, slot: Slot<CatalogModel[]>): Promise<void> {
    slot.loading = true;
    try {
      const result = await this.paseo.providers.listModels(provider);
      if (result.error && !result.models?.length) throw new Error(result.error);
      const next = (result.models ?? []).map((m) => ({ id: m.id, label: m.label, aliases: m.aliases }));
      slot.freshUntil = this.now() + MODEL_NAMES_TTL_MS;
      this.settle(slot, next);
    } catch (error) {
      slot.freshUntil = this.now() + MODEL_NAMES_RETRY_MS;
      this.log.warn("farol: model catalog lookup failed", provider, String(error));
    } finally {
      slot.loading = false;
    }
  }

  private settle<T>(slot: Slot<T>, next: T): void {
    const changed = JSON.stringify(next) !== JSON.stringify(slot.value);
    slot.value = next;
    if (changed && !this.closed) this.onChange();
  }
}
