import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS } from "../shared/settings";
import { type AgentFacts, digest, digestUsage, type Entry, MAX_AGENTS } from "../server/digest";
import { applyItem, emptyLive, foldItems } from "../server/live";
import { modelLabel, tzOffsetMinutes } from "../server/text";

const NOW = Date.parse("2026-11-02T06:00:00.000Z");

function agent(id: string, patch: Partial<AgentFacts> = {}): AgentFacts {
  return {
    id,
    title: id,
    provider: "claude",
    model: "claude-opus-5-5",
    status: "idle",
    updatedAt: "2026-11-02T05:59:00.000Z",
    pendingPermissions: [],
    ...patch,
  };
}

function run(entries: Entry[], patch: Partial<Parameters<typeof digest>[0]> = {}) {
  return digest({
    entries,
    usage: [],
    seen: {},
    focus: null,
    settings: DEFAULT_SETTINGS,
    host: "h",
    canStop: true,
    now: NOW,
    ...patch,
  });
}

const finished = (id: string) =>
  agent(id, { requiresAttention: true, attentionReason: "finished", attentionTimestamp: "2026-11-02T05:59:00.000Z" });

describe("digest", () => {
  it("derives states from Paseo's status, permissions and attention", () => {
    const states = run([
      { agent: agent("w", { status: "running", activeTurn: { turnId: "t", startedAt: null } }), project: null },
      { agent: agent("c", { pendingPermissions: [{ id: "p", name: "Bash", kind: "tool" }] }), project: null },
      { agent: agent("f", { requiresAttention: true, attentionReason: "error", lastError: "boom" }), project: null },
      { agent: finished("d"), project: null },
      { agent: agent("s"), project: null },
    ]).agents.map((a) => [a.id, a.state]);
    expect(states).toEqual([
      ["c", "call"],
      ["f", "fail"],
      ["d", "done"],
      ["w", "work"],
      ["s", "seen"],
    ]);
  });

  it("a board's seen mark holds until a newer turn ends", () => {
    const since = Date.parse("2026-11-02T05:59:00.000Z");
    expect(run([{ agent: finished("d"), project: null }], { seen: { d: since } }).agents[0]!.state).toBe("seen");
    expect(run([{ agent: finished("d"), project: null }], { seen: { d: since - 1 } }).agents[0]!.state).toBe("done");
  });

  it("read-only follows the allow-actions setting; previews follow theirs", () => {
    const live = foldItems([{ type: "assistant_message", text: "All done." }], null);
    const entries = [{ agent: finished("d"), project: null, live }];
    expect(run(entries).readOnly).toBe(false);
    expect(run(entries).agents[0]!.body).toBe("All done.");
    const quiet = run(entries, { settings: { ...DEFAULT_SETTINGS, allowActions: false, replyPreview: false } });
    expect(quiet.readOnly).toBe(true);
    expect(quiet.agents[0]!.body).toBeUndefined();
  });

  it("stoppable only for working agents when the CLI exists", () => {
    const w = { agent: agent("w", { status: "running" }), project: null };
    expect(run([w]).agents[0]!.stoppable).toBe(true);
    expect(run([w], { canStop: false }).agents[0]!.stoppable).toBe(false);
    expect(run([{ agent: finished("d"), project: null }]).agents[0]!.stoppable).toBeUndefined();
  });

  it("caps the deck and keeps the focused card", () => {
    const many: Entry[] = Array.from({ length: MAX_AGENTS + 6 }, (_, i) => ({
      agent: agent(`w${i}`, { status: "running" }),
      project: null,
    }));
    many.push({ agent: agent("old", { updatedAt: "2026-11-01T00:00:00.000Z" }), project: null });
    const out = run(many, { focus: "old" });
    expect(out.agents).toHaveLength(MAX_AGENTS);
    expect(out.agents.some((a) => a.id === "old")).toBe(true);
    expect(run(many).agents.some((a) => a.id === "old")).toBe(false);
  });

  it("the cap never hides waiting or working cards behind unread finished ones", () => {
    const entries: Entry[] = Array.from({ length: MAX_AGENTS + 4 }, (_, i) => ({
      agent: finished(`d${i}`),
      project: null,
    }));
    entries.push({ agent: agent("w", { status: "running" }), project: null });
    entries.push({ agent: agent("c", { pendingPermissions: [{ id: "p", name: "Bash", kind: "tool" }] }), project: null });
    const out = run(entries);
    expect(out.agents).toHaveLength(MAX_AGENTS);
    expect(out.agents[0]!.id).toBe("c");
    expect(out.agents.some((a) => a.id === "w")).toBe(true);
  });

  it("time zone: minutes east of UTC", () => {
    expect(tzOffsetMinutes("Asia/Shanghai", NOW)).toBe(480);
    expect(tzOffsetMinutes("America/Los_Angeles", NOW)).toBe(-480);
    expect(tzOffsetMinutes("UTC", NOW)).toBe(0);
    expect(tzOffsetMinutes("Not/AZone", NOW)).toBe(-new Date(NOW).getTimezoneOffset());
  });
});

describe("usage", () => {
  it("fuel is the tightest summary window, or the tightest of all when none is marked", () => {
    const usage = [
      {
        displayName: "A",
        status: "available",
        windows: [{ label: "5h", usedPct: 30 }, { label: "wk", remainingPct: 10 }],
      },
      { displayName: "B", status: "error", windows: [{ label: "x", usedPct: 99 }] },
    ];
    expect(digestUsage(usage)).toEqual({
      usage: [
        { label: "A · 5h", used: 30, resets: null },
        { label: "A · wk", used: 90, resets: null },
      ],
      fuel: { label: "A · wk", left: 10 },
    });
    usage[0]!.windows[0] = { label: "5h", usedPct: 30, summary: true } as never;
    expect(digestUsage(usage).fuel).toEqual({ label: "A · 5h", left: 70 });
  });
});

describe("live timeline", () => {
  it("updates tool calls in place and keeps the latest four", () => {
    let live = emptyLive();
    for (let i = 0; i < 6; i++) {
      live = applyItem(
        live,
        { type: "tool_call", callId: `c${i}`, name: "Bash", status: "running", detail: { type: "shell", command: `cmd ${i}` } },
        null,
        true,
      );
    }
    live = applyItem(live, { type: "tool_call", callId: "c5", name: "Bash", status: "completed", detail: { type: "shell", command: "cmd 5" } }, null, true);
    expect(live.actions.map((a) => [a.summary, a.running])).toEqual([
      ["cmd 2", true],
      ["cmd 3", true],
      ["cmd 4", true],
      ["cmd 5", false],
    ]);
  });

  it("joins streamed reply chunks and skips system errors", () => {
    let live = emptyLive();
    live = applyItem(live, { type: "assistant_message", text: "Hel" }, null, true);
    live = applyItem(live, { type: "assistant_message", text: "lo" }, null, true);
    live = applyItem(live, { type: "assistant_message", text: "[System Error] boom" }, null, true);
    expect(live.reply).toBe("Hello");
  });

  it("model labels", () => {
    expect(modelLabel("claude-opus-5-5")).toBe("opus 5.5");
    expect(modelLabel("gpt-6.1-sol")).toBe("gpt-6.1 sol");
    expect(modelLabel("codex/gpt-6.1-terra")).toBe("gpt-6.1 terra");
    expect(modelLabel(null)).toBe("");
  });
});
