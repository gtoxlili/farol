// Board long polls. Each board (device) has at most one waiting `farol.next`;
// it is answered as soon as that board's snapshot differs from the one it last
// received (including a focus change), or idle after POLL_MS. Model changes are
// coalesced to one flush per FLUSH_MS, and full snapshots to one board are
// spaced at least FLUSH_MS apart: at most 5 per second.
import type { BoardPrefs, Snapshot } from "../shared/protocol";
import type { Board } from "../shared/settings";

export const POLL_MS = 25_000;
export const FLUSH_MS = 200;
/** A board with no poll for this long is shown offline. */
export const ONLINE_MS = 35_000;
const MAX_DEVICES = 16;

type Body = Omit<Snapshot, "rev" | "idle">;

interface Waiter {
  resolve(snapshot: Snapshot): void;
  timer: ReturnType<typeof setTimeout>;
}

interface Device {
  id: string;
  focus: string | null;
  firstSeen: number;
  lastSeen: number;
  /** The rev and content key of the last full snapshot sent. */
  sentRev: number;
  sentKey: string;
  /** When the last full snapshot went out; full answers are spaced by FLUSH_MS. */
  sentAt: number;
  waiter: Waiter | null;
  /** A delayed wake to respect the push rate. */
  wakeTimer: ReturnType<typeof setTimeout> | null;
  /** The board's own control-centre settings, as last reported. */
  prefs: BoardPrefs | null;
}

export interface StreamOptions {
  /** Builds the snapshot body for a focus. */
  build(focus: string | null): Body;
  clock?: () => number;
  pollMs?: number;
  flushMs?: number;
}

/** Content identity of a snapshot body, ignoring the clock. */
function keyOf(body: Body): string {
  const { now: _now, ...rest } = body;
  return JSON.stringify(rest);
}

export class Stream {
  private readonly devices = new Map<string, Device>();
  private readonly clock: () => number;
  private readonly pollMs: number;
  private readonly flushMs: number;
  /** Starts at the clock so a restarted plugin never reuses an older cursor. */
  private rev: number;
  private lastFlush = 0;
  private flushTimer: ReturnType<typeof setTimeout> | null = null;
  private closed = false;

  constructor(private readonly options: StreamOptions) {
    this.clock = options.clock ?? Date.now;
    this.pollMs = options.pollMs ?? POLL_MS;
    this.flushMs = options.flushMs ?? FLUSH_MS;
    this.rev = this.clock();
  }

  /** Long poll. Resolves at once when the board is behind, else on change or timeout. */
  next(device: string, cursor: number, focus: string | null | undefined): Promise<Snapshot> {
    const d = this.device(device);
    if (focus !== undefined) d.focus = focus ?? null;
    // A newer poll from the same board replaces the old one.
    this.release(d, true);
    const body = this.options.build(d.focus);
    const key = keyOf(body);
    // A board that is behind (fresh start, reconnect, plugin restart) gets an answer at once.
    if (cursor !== d.sentRev || this.closed) {
      return Promise.resolve(this.full(d, body, key));
    }
    const promise = new Promise<Snapshot>((resolve) => {
      const timer = setTimeout(() => {
        if (d.waiter?.timer !== timer) return;
        d.waiter = null;
        d.lastSeen = this.clock();
        resolve(this.idle(d));
      }, this.pollMs);
      d.waiter = { resolve, timer };
    });
    if (key !== d.sentKey) this.wake(d);
    return promise;
  }

  /** The board's card changed: answer its waiting poll with the new focus. */
  focus(device: string, focus: string | null): void {
    const d = this.device(device);
    if (d.focus === focus) return;
    d.focus = focus;
    this.wake(d);
  }

  /** Something in the model changed; flushes at most every `flushMs`. */
  changed(): void {
    if (this.flushTimer || this.closed) return;
    const wait = Math.max(0, this.lastFlush + this.flushMs - this.clock());
    this.flushTimer = setTimeout(() => {
      this.flushTimer = null;
      this.lastFlush = this.clock();
      this.flush();
    }, wait);
  }

  /** Answers every waiting poll whose snapshot changed. */
  flush(): void {
    for (const d of this.devices.values()) this.wake(d);
  }

  /** Records what a board's control centre reports (it resends until a poll is answered). */
  setPrefs(device: string, prefs: BoardPrefs): void {
    this.device(device).prefs = prefs;
  }

  /** Focus of every board seen recently, for the observer's live subscriptions. */
  focuses(): string[] {
    const now = this.clock();
    const out = new Set<string>();
    for (const d of this.devices.values()) {
      if (d.focus && (d.waiter || now - d.lastSeen < ONLINE_MS)) out.add(d.focus);
    }
    return [...out];
  }

  boards(): Board[] {
    const now = this.clock();
    return [...this.devices.values()]
      .sort((a, b) => b.lastSeen - a.lastSeen)
      .map((d) => ({
        device: d.id,
        firstSeen: d.firstSeen,
        lastSeen: d.lastSeen,
        online: d.waiter !== null || now - d.lastSeen < ONLINE_MS,
        focus: d.focus,
        prefs: d.prefs,
      }));
  }

  /** Answers all waiting polls idle and stops timers. */
  close(): void {
    this.closed = true;
    if (this.flushTimer) clearTimeout(this.flushTimer);
    this.flushTimer = null;
    for (const d of this.devices.values()) {
      if (d.wakeTimer) clearTimeout(d.wakeTimer);
      d.wakeTimer = null;
      this.release(d, true);
    }
  }

  private wake(d: Device): void {
    if (!d.waiter || d.wakeTimer) return;
    const wait = d.sentAt + this.flushMs - this.clock();
    if (wait > 0) {
      d.wakeTimer = setTimeout(() => {
        d.wakeTimer = null;
        this.wake(d);
      }, wait);
      return;
    }
    const body = this.options.build(d.focus);
    const key = keyOf(body);
    if (key === d.sentKey) return;
    const waiter = d.waiter;
    d.waiter = null;
    clearTimeout(waiter.timer);
    waiter.resolve(this.full(d, body, key));
  }

  private full(d: Device, body: Body, key: string): Snapshot {
    if (key !== d.sentKey || d.sentRev === 0) this.rev = Math.max(this.rev + 1, d.sentRev + 1);
    d.sentRev = this.rev;
    d.sentKey = key;
    d.sentAt = this.clock();
    d.lastSeen = d.sentAt;
    return { ...body, rev: this.rev };
  }

  private idle(d: Device): Snapshot {
    const body = this.options.build(d.focus);
    return { ...body, rev: d.sentRev, idle: true, agents: [], usage: [] };
  }

  private release(d: Device, answer: boolean): void {
    const waiter = d.waiter;
    if (!waiter) return;
    d.waiter = null;
    clearTimeout(waiter.timer);
    if (answer) waiter.resolve(this.idle(d));
  }

  private device(id: string): Device {
    let d = this.devices.get(id);
    const now = this.clock();
    if (!d) {
      if (this.devices.size >= MAX_DEVICES) this.evict();
      d = {
        id,
        focus: null,
        firstSeen: now,
        lastSeen: now,
        sentRev: 0,
        sentKey: "",
        sentAt: 0,
        waiter: null,
        wakeTimer: null,
        prefs: null,
      };
      this.devices.set(id, d);
    }
    d.lastSeen = now;
    return d;
  }

  private evict(): void {
    let oldest: Device | null = null;
    for (const d of this.devices.values()) {
      if (!d.waiter && (!oldest || d.lastSeen < oldest.lastSeen)) oldest = d;
    }
    if (oldest) this.devices.delete(oldest.id);
  }
}
