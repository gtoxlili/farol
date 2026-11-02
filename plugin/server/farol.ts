// Composition root of the server side. Owns the observer, the board streams,
// the action policy, the settings view and the per-agent "seen" marks. The
// Paseo API arrives with the first RPC or hook (plugin contexts carry it), so
// observation starts lazily and lives as long as the plugin process.
import { execFile } from "node:child_process";
import os from "node:os";
import type { PaseoApi } from "@getpaseo/client";
import type { Act, ActResult, BoardPrefs, BoardSettings, Snapshot } from "../shared/protocol";
import { DEFAULT_SETTINGS, type Settings, TIMELINE_KIND } from "../shared/settings";
import { Actions } from "./actions";
import type { AgentFacts } from "./digest";
import { boardSettingsOf, digest } from "./digest";
import { Observer } from "./observer";
import { findPaseoCli, runStop } from "./stop";
import { Stream } from "./stream";

const MAX_SEEN = 512;

export class Farol {
  readonly stream: Stream;
  private readonly actions: Actions;
  private observer: Observer | null = null;
  private starting: Promise<Observer> | null = null;
  private settings: Settings = DEFAULT_SETTINGS;
  /** Board settings as last sent, and their rev (starts at the clock, so it survives restarts). */
  private boardSettings: BoardSettings = { ...boardSettingsOf(DEFAULT_SETTINGS, Date.now()) };
  private readonly seen = new Map<string, number>();
  private readonly cli: string | null = findPaseoCli();
  private host = os.hostname().replace(/\.local$/, "");
  private closed = false;

  constructor(private readonly log: { info(...a: unknown[]): void; warn(...a: unknown[]): void } = console) {
    this.stream = new Stream({ build: (focus) => this.build(focus) });
    this.actions = new Actions({
      agent: (id) => (this.observer?.get(id) as unknown as AgentFacts | null) ?? null,
      allowActions: () => this.settings.allowActions,
      canStop: () => this.cli !== null,
      respond: async (agentId, requestId, response) => {
        const paseo = await this.paseo();
        await paseo.agents.ref(agentId).respondToPermission({ requestId, response });
      },
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

  /** Starts observing with the API from a plugin context; idempotent. */
  ensure(paseo: PaseoApi): Promise<Observer> {
    if (this.observer) return Promise.resolve(this.observer);
    if (!this.starting) {
      this.apiValue = paseo;
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

  setSettings(next: Settings): void {
    const background = next.includeBackground !== this.settings.includeBackground;
    this.settings = next;
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
    });
  }
}
