import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Snapshot } from "../shared/protocol";
import { FLUSH_MS, POLL_MS, Stream } from "../server/stream";

type Body = Omit<Snapshot, "rev" | "idle">;

function setup() {
  let agents: Body["agents"] = [];
  const builds: Array<string | null> = [];
  const stream = new Stream({
    build: (focus) => {
      builds.push(focus);
      return {
        v: 1,
        now: Date.now(),
        tz: 0,
        host: "h",
        readOnly: false,
        usage: [],
        agents: agents.map((a) => (a.id === focus ? { ...a, now: "detail" } : a)),
      };
    },
  });
  const set = (ids: string[]) => {
    agents = ids.map((id) => ({ id, state: "work" as const }));
    stream.changed();
  };
  return { stream, set, builds };
}

async function settled<T>(p: Promise<T>): Promise<T | "pending"> {
  return Promise.race([p, Promise.resolve().then(() => "pending" as const)]);
}

describe("Stream", () => {
  beforeEach(() => vi.useFakeTimers({ now: 1_000_000 }));
  afterEach(() => vi.useRealTimers());

  it("answers a board that is behind at once", async () => {
    const { stream } = setup();
    const s = await stream.next("b", 0, null);
    expect(s.idle).toBeUndefined();
    expect(s.rev).toBeGreaterThan(0);
  });

  it("waits while nothing changed, then answers idle at POLL_MS with the same rev", async () => {
    const { stream } = setup();
    const first = await stream.next("b", 0, null);
    const poll = stream.next("b", first.rev, null);
    expect(await settled(poll)).toBe("pending");
    await vi.advanceTimersByTimeAsync(POLL_MS - 1);
    expect(await settled(poll)).toBe("pending");
    await vi.advanceTimersByTimeAsync(1);
    const idle = await poll;
    expect(idle.idle).toBe(true);
    expect(idle.v).toBe(1);
    expect(idle.rev).toBe(first.rev);
    expect(POLL_MS).toBeLessThan(30_000);
  });

  it("answers a waiting poll when the model changes, with a higher rev", async () => {
    const { stream, set } = setup();
    const first = await stream.next("b", 0, null);
    await vi.advanceTimersByTimeAsync(FLUSH_MS);
    const poll = stream.next("b", first.rev, null);
    set(["a"]);
    await vi.advanceTimersByTimeAsync(FLUSH_MS);
    const s = await poll;
    expect(s.agents.map((a) => a.id)).toEqual(["a"]);
    expect(s.rev).toBeGreaterThan(first.rev);
  });

  it("a change that does not alter this board's snapshot keeps it waiting", async () => {
    const { stream } = setup();
    const first = await stream.next("b", 0, null);
    const poll = stream.next("b", first.rev, null);
    stream.changed();
    await vi.advanceTimersByTimeAsync(FLUSH_MS * 2);
    expect(await settled(poll)).toBe("pending");
  });

  it("focus wakes the waiting poll with the focused card's detail", async () => {
    const { stream, set } = setup();
    set(["a", "b"]);
    const first = await stream.next("dev", 0, null);
    await vi.advanceTimersByTimeAsync(FLUSH_MS);
    const poll = stream.next("dev", first.rev, null);
    stream.focus("dev", "b");
    const s = await poll;
    expect(s.agents.find((a) => a.id === "b")?.now).toBe("detail");
    expect(s.agents.find((a) => a.id === "a")?.now).toBeUndefined();
  });

  it("sends at most 5 full snapshots per second to a board", async () => {
    const { stream, set } = setup();
    let cursor = (await stream.next("b", 0, null)).rev;
    const sent: number[] = [];
    let n = 0;
    const loop = (async () => {
      for (let i = 0; i < 12; i++) {
        const s = await stream.next("b", cursor, null);
        cursor = s.rev;
        if (!s.idle) sent.push(Date.now());
      }
    })();
    for (let t = 0; t < 3000; t += 20) {
      set([`a${n++}`]);
      await vi.advanceTimersByTimeAsync(20);
    }
    await vi.advanceTimersByTimeAsync(POLL_MS * 2);
    await loop;
    for (let i = 0; i + 5 < sent.length; i++) {
      expect(sent[i + 5]! - sent[i]!).toBeGreaterThanOrEqual(1000);
    }
    expect(sent.length).toBeGreaterThan(5);
  });

  it("a newer poll from the same board releases the older one idle", async () => {
    const { stream } = setup();
    const first = await stream.next("b", 0, null);
    const old = stream.next("b", first.rev, null);
    const fresh = stream.next("b", first.rev, null);
    expect((await old).idle).toBe(true);
    expect(await settled(fresh)).toBe("pending");
  });

  it("lists boards with their focus and online state", async () => {
    const { stream } = setup();
    await stream.next("b", 0, "x");
    expect(stream.boards()).toMatchObject([{ device: "b", online: true, focus: "x" }]);
    expect(stream.focuses()).toEqual(["x"]);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(stream.boards()[0]!.online).toBe(false);
    expect(stream.focuses()).toEqual([]);
  });
});
