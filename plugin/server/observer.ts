// Keeps an in-memory picture of the daemon's agents through the public SDK:
// the agent directory subscription, timeline streams for running and focused
// agents, a tail refetch when a turn ends, and provider usage every few minutes.
// Owned by the plugin's server entry; single-threaded (Node event loop).
import type { PaseoAgent, PaseoApi } from "@getpaseo/client";
import type { AgentFacts, Entry, UsageFacts } from "./digest";
import { applyItem, emptyLive, foldItems, type Live, type TimelineItem } from "./live";

export const USAGE_EVERY_MS = 5 * 60 * 1000;
/** A turn end refreshes usage too, at most this often (the daemon caches vendor fetches). */
const USAGE_AFTER_TURN_MS = 60 * 1000;
const TAIL_LIMIT = 40;
const LIST_LIMIT = 100;
/** How long a focus change waits for the focused agent's history before answering. */
export const FOCUS_FETCH_MS = 1500;

interface Logger {
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
}

interface Tracked {
  agent: PaseoAgent;
  project: string | null;
  live: Live | null;
  /** Timeline stream, while the agent runs or a board looks at it. */
  stream: (() => void) | null;
  /** Pending history fetch. */
  fetching: Promise<void> | null;
}

export class Observer {
  private readonly agents = new Map<string, Tracked>();
  private usage: UsageFacts[] = [];
  private usageAt = 0;
  private listRelease: (() => void) | null = null;
  private usageTimer: ReturnType<typeof setInterval> | null = null;
  private wanted = new Set<string>();
  private includeBackground = false;
  private generation = 0;
  private closed = false;

  constructor(
    private readonly paseo: PaseoApi,
    private readonly onChange: () => void,
    private readonly log: Logger = console,
  ) {}

  async start(includeBackground: boolean): Promise<void> {
    this.includeBackground = includeBackground;
    await this.subscribe();
    void this.refreshUsage();
    this.usageTimer = setInterval(() => void this.refreshUsage(), USAGE_EVERY_MS);
  }

  /** Resubscribes when the background-workspace setting changes. */
  async setIncludeBackground(value: boolean): Promise<void> {
    if (value === this.includeBackground) return;
    this.includeBackground = value;
    await this.subscribe();
  }

  entries(): Entry[] {
    return [...this.agents.values()].map((t) => ({
      agent: t.agent as unknown as AgentFacts,
      project: t.project,
      live: t.live,
    }));
  }

  usageFacts(): UsageFacts[] {
    return this.usage;
  }

  get(id: string): PaseoAgent | null {
    return this.agents.get(id)?.agent ?? null;
  }

  /**
   * Agents the boards are looking at. Running agents are always followed; these
   * are followed too. Resolves once their history is loaded (or after a bound).
   */
  async setFocused(ids: readonly string[]): Promise<void> {
    this.wanted = new Set(ids);
    const loads: Promise<void>[] = [];
    for (const id of ids) {
      const t = this.agents.get(id);
      if (t) loads.push(this.follow(t));
    }
    this.reconcile();
    if (loads.length) {
      await Promise.race([
        Promise.allSettled(loads),
        new Promise((r) => setTimeout(r, FOCUS_FETCH_MS)),
      ]);
    }
  }

  /** A turn ended or a permission changed: fetch the snapshot and the tail. */
  async refreshAgent(id: string): Promise<void> {
    const t = this.agents.get(id);
    if (!t) return;
    await this.loadHistory(t);
    this.onChange();
    if (Date.now() - this.usageAt > USAGE_AFTER_TURN_MS) void this.refreshUsage();
  }

  async refreshUsage(): Promise<void> {
    this.usageAt = Date.now();
    try {
      const result = await this.paseo.providers.listUsage();
      this.usage = result.providers.map((p) => ({
        displayName: p.displayName,
        status: p.status,
        windows: p.windows,
      }));
      this.onChange();
    } catch (error) {
      this.log.warn("farol: usage refresh failed", String(error));
    }
  }

  async close(): Promise<void> {
    this.closed = true;
    if (this.usageTimer) clearInterval(this.usageTimer);
    this.listRelease?.();
    this.listRelease = null;
    for (const t of this.agents.values()) this.unfollow(t);
    this.agents.clear();
  }

  private async subscribe(): Promise<void> {
    const generation = ++this.generation;
    this.listRelease?.();
    this.listRelease = null;
    const first = await this.paseo.agents.list({
      subscribe: {},
      scope: "active",
      includeBackground: this.includeBackground,
      // Newest first; the board shows at most a couple of dozen cards.
      sort: [{ key: "updated_at", direction: "desc" }],
      page: { limit: LIST_LIMIT },
    });
    const subscription = first.subscription;
    if (generation !== this.generation || this.closed) {
      void subscription.release();
      return;
    }
    const replace = (entries: typeof first.entries) => {
      const seen = new Set<string>();
      for (const { agent, project } of entries) {
        seen.add(agent.id);
        this.upsert(agent, project?.projectName ?? null);
      }
      for (const [id, t] of this.agents) {
        if (!seen.has(id)) {
          this.unfollow(t);
          this.agents.delete(id);
        }
      }
      this.reconcile();
      this.onChange();
    };
    replace(first.entries);
    const unobserve = subscription.subscribe({
      snapshot: ({ entries }) => replace(entries),
      update: (message) => {
        if (message.type !== "agent_update") return;
        const update = message.payload;
        if (update.kind === "upsert") {
          this.upsert(update.agent, update.project?.projectName ?? null);
        } else {
          const t = this.agents.get(update.agentId);
          if (t) this.unfollow(t);
          this.agents.delete(update.agentId);
        }
        this.reconcile();
        this.onChange();
      },
      error: (error) => this.log.warn("farol: agent subscription error", String(error)),
    });
    this.listRelease = () => {
      unobserve();
      void subscription.release();
    };
  }

  private upsert(agent: PaseoAgent, project: string | null): void {
    const t = this.agents.get(agent.id);
    if (t) {
      const turnEnded = t.agent.status === "running" && agent.status !== "running";
      t.agent = agent;
      if (project) t.project = project;
      if (turnEnded) void this.loadHistory(t).then(() => this.onChange());
    } else {
      const fresh: Tracked = { agent, project, live: null, stream: null, fetching: null };
      this.agents.set(agent.id, fresh);
      // Finished cards show the last reply, so load their tail once.
      if (agent.requiresAttention || agent.status === "running") {
        void this.loadHistory(fresh).then(() => this.onChange());
      }
    }
  }

  /** Streams exactly the running and focused agents. */
  private reconcile(): void {
    for (const t of this.agents.values()) {
      const want = t.agent.status === "running" || this.wanted.has(t.agent.id);
      if (want && !t.stream) void this.follow(t);
      else if (!want && t.stream) this.unfollow(t);
    }
  }

  private follow(t: Tracked): Promise<void> {
    if (!t.stream && !this.closed) {
      const id = t.agent.id;
      const unsubscribe = this.paseo.agents.ref(id).timeline.subscribe((event) => {
        const current = this.agents.get(id);
        if (!current || !("event" in event)) return;
        const e = event.event;
        if (e.type === "timeline") {
          current.live = applyItem(current.live ?? emptyLive(), e.item as TimelineItem, current.agent.cwd, true);
          this.onChange();
        } else if (e.type === "replacement") {
          void this.loadHistory(current).then(() => this.onChange());
        }
      });
      t.stream = () => unsubscribe();
      unsubscribe.ready.catch((error) => {
        this.log.warn("farol: timeline subscribe failed", id, String(error));
        if (t.stream) t.stream = null;
      });
    }
    return t.live ? Promise.resolve() : this.loadHistory(t);
  }

  private unfollow(t: Tracked): void {
    t.stream?.();
    t.stream = null;
  }

  private loadHistory(t: Tracked): Promise<void> {
    if (t.fetching) return t.fetching;
    const id = t.agent.id;
    t.fetching = this.paseo.agents
      .ref(id)
      .timeline.refetch({ direction: "tail", limit: TAIL_LIMIT, projection: "projected" })
      .then((page) => {
        const current = this.agents.get(id);
        if (!current) return;
        current.live = foldItems(
          page.entries.map((e) => e.item as TimelineItem),
          current.agent.cwd,
        );
      })
      .catch((error) => this.log.warn("farol: timeline fetch failed", id, String(error)))
      .finally(() => {
        t.fetching = null;
      });
    return t.fetching;
  }
}
