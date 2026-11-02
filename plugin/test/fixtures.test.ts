// The shared protocol fixtures, run through the same digest the plugin serves.
// rust/logic/src/model.rs parses protocol/fixtures/snapshot.json; this test
// proves the plugin produces that snapshot from Paseo-shaped facts.
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { SnapshotSchema, STATES } from "../shared/protocol";
import { DEFAULT_SETTINGS } from "../shared/settings";
import { boardSettingsOf, digest, type Entry } from "../server/digest";
import { foldItems, type TimelineItem } from "../server/live";

const FIXTURES = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../protocol/fixtures");
const fixture = (name: string) => JSON.parse(readFileSync(path.join(FIXTURES, name), "utf8"));

const input = fixture("digest-input.json");
const expected = fixture("snapshot.json");

function run(overrides: Record<string, unknown> = {}) {
  const entries: Entry[] = input.entries.map(
    (e: { agent: Entry["agent"] & { cwd?: string }; project: string; timeline: TimelineItem[] }) => ({
      agent: e.agent,
      project: e.project,
      live: foldItems(e.timeline, e.agent.cwd ?? null),
    }),
  );
  return digest({
    entries,
    usage: input.usage,
    seen: input.seen,
    focus: input.focus,
    settings: { ...DEFAULT_SETTINGS, ...input.settings },
    host: input.host,
    canStop: input.canStop,
    now: input.now,
    boardSettings: boardSettingsOf({ ...DEFAULT_SETTINGS, ...input.settings }, input.settingsRev),
    ...overrides,
  });
}

describe("protocol fixtures", () => {
  it("snapshot.json is a valid farol/1 snapshot", () => {
    expect(SnapshotSchema.safeParse(expected).success).toBe(true);
  });

  it("digest-input.json digests to snapshot.json", () => {
    const out = { ...run(), rev: input.rev };
    expect(SnapshotSchema.parse(out)).toEqual(out);
    const byId = (s: { agents: Array<{ id: string }> }) =>
      Object.fromEntries(s.agents.map((a) => [a.id, a]));
    expect(byId(out)).toEqual(byId(expected));
    const { agents: _a, ...head } = out;
    const { agents: _b, ...want } = expected;
    expect(head).toEqual(want);
  });

  it("orders cards by urgency: call, fail, done, work, seen", () => {
    const ranks = run().agents.map((a) => STATES.indexOf(a.state));
    expect(ranks).toEqual([...ranks].sort((x, y) => x - y));
    expect(run().agents.map((a) => a.id)).toEqual(["a-call", "a-fail", "a-done", "a-work"]);
  });

  it("only the focused working card carries recent actions and todo", () => {
    const unfocused = run({ focus: null }).agents.find((a) => a.id === "a-work")!;
    expect(unfocused.now).toBe("cargo test -p runtime --lib");
    expect(unfocused.recent).toBeUndefined();
    expect(unfocused.todo).toBeUndefined();
  });
});
