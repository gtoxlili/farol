// Composition root of the server side. Owns the observer, the board streams,
// the action policy, the settings view and the per-agent "seen" marks. The
// Paseo API arrives with the first RPC or hook (plugin contexts carry it), so
// observation starts lazily and lives as long as the plugin process.
import { execFile } from "node:child_process";
import os from "node:os";
import type { PaseoApi } from "@getpaseo/client";
import type { Act, ActResult, BoardPrefs, BoardSettings, Snapshot, Start } from "../shared/protocol";
import { DEFAULT_SETTINGS, type Settings, TIMELINE_KIND } from "../shared/settings";
import { Actions, type PermissionResponse } from "./actions";
import type { AgentFacts } from "./digest";
import { boardSettingsOf, digest } from "./digest";
import { Observer } from "./observer";
import { boardLanguage, type ProfileFacts, type StartPorts, startSession } from "./start";
import { findPaseoCli, runStop } from "./stop";
import { Stream } from "./stream";

const MAX_SEEN = 512;

export class Farol {
  readonly stream: Stream;
  private readonly actions: Actions;
  private observer: Observer | null = null;
  private starting: Promise<Observer> | null = null;
  private settings: Settings = DEFAULT_SETTINGS;
  /** Answers waiting to learn which response Paseo applied, by request id. */
  private readonly resolutions = new Map<string, Array<(r: PermissionResponse | null) => void>>();
  /** Board settings as last sent, and their rev (starts at the clock, so it survives restarts). */
  private boardSettings: BoardSettings = { ...boardSettingsOf(DEFAULT_SETTINGS, Date.now()) };
  private readonly seen = new Map<string, number>();
  private readonly cli: string | null = findPaseoCli();
  private host = os.hostname().replace(/\.local$/, "");
  private closed = false;
  /** The Paseo app's language, as its client last reported (null until one does). */
  private appLocale: string | null = null;
  /** The default workspace's name, for the boards' new-session page. */
  private workspaceName: string | null = null;
  private readonly starter: StartPorts;

  constructor(private readonly log: { info(...a: unknown[]): void; warn(...a: unknown[]): void } = console) {
    this.stream = new Stream({ build: (focus) => this.build(focus) });
    this.actions = new Actions({
      agent: (id) => (this.observer?.get(id) as unknown as AgentFacts | null) ?? null,
      allowActions: () => this.settings.allowActions,
      canStop: () => this.cli !== null,
      fresh: async (agentId) => {
        const paseo = await this.paseo();
        const ref = paseo.agents.ref(agentId);
        await ref.refresh();
        return (ref.current() as unknown as AgentFacts | null) ?? null;
      },
      respond: async (agentId, requestId, response) => {
        const paseo = await this.paseo();
        await paseo.agents.ref(agentId).respondToPermission({ requestId, response });
      },
      resolution: (requestId, timeoutMs) =>
        new Promise((resolve) => {
          const waiters = this.resolutions.get(requestId) ?? [];
          const timer = setTimeout(() => settle(null), timeoutMs);
          const settle = (r: PermissionResponse | null) => {
            clearTimeout(timer);
            resolve(r);
          };
          waiters.push(settle);
          this.resolutions.set(requestId, waiters);
        }),
      stop: (agentId) => runStop(this.cli!, agentId),
      appendRow: async (agentId, rowId, row) => {
        const paseo = await this.paseo();
        await paseo.agents
          .ref(agentId)
          .timeline.append({ type: "plugin", id: rowId, kind: TIMELINE_KIND, version: 1, data: row });
      },
      markSeen: (agentId, since) => {
        this.seen.delete(agentId);
        this.seen.set(agentId, since);
        while (this.seen.size > MAX_SEEN) this.seen.delete(this.seen.keys().next().value!);
        this.stream.changed();
      },
      log: this.log,
    });
    this.starter = {
      settings: () => this.settings,
      profile: async (id) => {
        const { config } = await (await this.paseo()).config.get();
        const found = config.agentProfiles?.find((p) => p.id === id);
        return (found as ProfileFacts | undefined) ?? null;
      },
      defaultModel: async (provider) => {
        // Discovery is lazy: right after a (re)start the snapshot has no models yet.
        const snapshot = await (await this.paseo()).providers.waitForReady();
        const entry = snapshot.entries.find((e) => e.provider === provider);
        const models = entry?.models ?? [];
        return (models.find((m) => m.isDefault) ?? models[0])?.id ?? null;
      },
      create: async (workspaceId, input) => {
        const agent = await (await this.paseo()).workspaces.ref(workspaceId).agents.create(input);
        return { id: agent.id };
      },
      log: this.log,
    };
    if (process.platform === "darwin") {
      execFile("scutil", ["--get", "ComputerName"], { timeout: 2000 }, (error, stdout) => {
        const name = stdout?.trim();
        if (!error && name) {
          this.host = name;
          this.stream.changed();
        }
      });
    }
  }

  get canStop(): boolean {
    return this.cli !== null;
  }

  get usageAt(): number | null {
    return this.observer?.usageFetchedAt || null;
  }

  get hostLabel(): string {
    return this.host;
  }

  /** The language boards use now, or null while unknown. */
  get boardLanguage(): "zh" | "en" | null {
    return boardLanguage(this.settings.boardLanguage, this.appLocale) ?? null;
  }

  /** A Paseo app reported its language; boards on "auto" follow the latest. */
  setLocale(locale: string): void {
    if (locale === this.appLocale) return;
    this.appLocale = locale;
    this.stream.changed();
  }

  /** `farol.start`: a new session from a board. */
  async start(paseo: PaseoApi, input: Start): Promise<ActResult> {
    await this.ensure(paseo);
    const result = await startSession(this.starter, input);
    if (result.ok) this.stream.changed();
    return result;
  }

  /** Looks up the default workspace's name for the boards. */
  private async refreshWorkspace(): Promise<void> {
    const id = this.settings.workspaceId;
    if (!id || !this.apiValue) {
      this.workspaceName = null;
      return;
    }
    try {
      const workspace = await this.apiValue.workspaces.ref(id).refresh();
      if (this.settings.workspaceId !== id) return;
      this.workspaceName = workspace?.name ?? null;
    } catch (error) {
      this.log.warn("farol: workspace lookup failed", String(error));
      this.workspaceName = null;
    }
    this.stream.changed();
  }

  /** Starts observing with the API from a plugin context; idempotent. */
  ensure(paseo: PaseoApi): Promise<Observer> {
    if (this.observer) return Promise.resolve(this.observer);
    if (!this.starting) {
      this.apiValue = paseo;
      void this.refreshWorkspace();
      const observer = new Observer(paseo, () => this.stream.changed(), this.log);
      this.starting = observer
        .start(this.settings.includeBackground)
        .then(() => {
          if (this.closed) {
            void observer.close();
          } else {
            this.observer = observer;
          }
          return observer;
        })
        .catch((error) => {
          this.starting = null;
          throw error;
        });
    }
    return this.starting;
  }

  /** agent.permission_resolved: wakes the board answer waiting on this request. */
  resolved(requestId: string, resolution: PermissionResponse): void {
    const waiters = this.resolutions.get(requestId);
    this.resolutions.delete(requestId);
    for (const settle of waiters ?? []) settle(resolution);
  }

  setSettings(next: Settings): void {
    const background = next.includeBackground !== this.settings.includeBackground;
    const workspace = next.workspaceId !== this.settings.workspaceId;
    this.settings = next;
    if (workspace) void this.refreshWorkspace();
    if (background) void this.observer?.setIncludeBackground(next.includeBackground);
    this.updateBoardSettings();
  }

  /**
   * A board's control centre reported its settings. They stay the board's own
   * (it keeps them in flash) and show in the recent-boards list; the settings
   * made in Paseo reach every board through `settings.rev`. Plugin server code
   * cannot write the settings document (Paseo 0.14), so nothing is held here
   * that a plugin restart could lose.
   */
  notePrefs(device: string, prefs: BoardPrefs | undefined): void {
    if (prefs) this.stream.setPrefs(device, prefs);
  }

  /** A board turned to its usage screen: fresh quota numbers now (at most once a minute). */
  private noteScreen(observer: Observer, device: string, meta: { fw?: string; screen?: string }): void {
    if (this.stream.setMeta(device, meta) && meta.screen === "usage") observer.refreshUsageSoon();
  }

  private updateBoardSettings(): void {
    const next = boardSettingsOf(this.settings, this.boardSettings.rev);
    if (JSON.stringify(next) !== JSON.stringify(this.boardSettings)) {
      this.boardSettings = { ...next, rev: this.boardSettings.rev + 1 };
    }
    this.stream.changed();
  }

  async next(
    paseo: PaseoApi,
    device: string,
    cursor: number,
    focus: string | null | undefined,
    prefs?: BoardPrefs,
    meta: { fw?: string; screen?: string } = {},
  ): Promise<Snapshot> {
    this.notePrefs(device, prefs);
    const observer = await this.ensure(paseo);
    this.noteScreen(observer, device, meta);
    if (focus !== undefined) await observer.setFocused(this.focusesWith(device, focus ?? null));
    return this.stream.next(device, cursor, focus);
  }

  async focus(
    paseo: PaseoApi,
    device: string,
    focus: string | null,
    prefs?: BoardPrefs,
    screen?: string,
  ): Promise<void> {
    this.notePrefs(device, prefs);
    const observer = await this.ensure(paseo);
    this.noteScreen(observer, device, { screen });
    await observer.setFocused(this.focusesWith(device, focus));
    this.stream.focus(device, focus);
  }

  async act(paseo: PaseoApi, input: Act, device: string): Promise<ActResult> {
    await this.ensure(paseo);
    return this.actions.act(input, device);
  }

  /** A hook saw a turn end: reload that agent's tail for the reply preview. */
  turnEnded(paseo: PaseoApi, agentId: string): void {
    void this.ensure(paseo).then((o) => o.refreshAgent(agentId));
  }

  async close(): Promise<void> {
    this.closed = true;
    this.stream.close();
    await this.observer?.close();
    this.observer = null;
  }

  private apiValue: PaseoApi | null = null;

  private async paseo(): Promise<PaseoApi> {
    if (!this.apiValue) throw new Error("Paseo API not available yet");
    return this.apiValue;
  }

  private focusesWith(device: string, focus: string | null): string[] {
    const others = this.stream
      .boards()
      .filter((b) => b.device !== device && b.online && b.focus)
      .map((b) => b.focus!);
    return focus ? [...others, focus] : others;
  }

  private build(focus: string | null) {
    return digest({
      entries: this.observer?.entries() ?? [],
      usage: this.observer?.usageFacts() ?? [],
      seen: Object.fromEntries(this.seen),
      focus,
      settings: this.settings,
      host: this.host,
      canStop: this.cli !== null,
      now: Date.now(),
      boardSettings: this.boardSettings,
      lang: boardLanguage(this.settings.boardLanguage, this.appLocale),
      compose:
        this.settings.allowActions && this.settings.workspaceId && this.settings.profileId
          ? { workspace: this.workspaceName ?? this.settings.workspaceId }
          : undefined,
    });
  }
}
