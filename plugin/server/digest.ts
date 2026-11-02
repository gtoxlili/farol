// Paseo facts -> farol/1 snapshot. Pure: everything it needs comes in `DigestInput`,
// so the shared protocol fixtures drive it directly in tests.
import type { Agent, Ask, BoardSettings, Fuel, Snapshot, State, Window } from "../shared/protocol";
import { PROTOCOL_VERSION, STATES } from "../shared/protocol";
import type { Settings } from "../shared/settings";
import type { Live } from "./live";
import { BODY_MAX, farolMarkdown } from "./markdown";
import { type ModelDirectory, modelLabelFor } from "./models";
import { clip, epochMs, formatCost, tzOffsetMinutes } from "./text";

/** The parts of a Paseo permission request the board shows. */
export interface PermissionFacts {
  id: string;
  name: string;
  kind: string;
  title?: string;
  description?: string;
  input?: Record<string, unknown>;
  detail?: { type: string; [key: string]: unknown };
  actions?: Array<{ id: string; label: string; behavior: "allow" | "deny" }>;
}

/** The parts of Paseo's AgentSnapshotPayload the digest reads. */
export interface AgentFacts {
  id: string;
  title: string | null;
  provider: string;
  model: string | null;
  cwd?: string;
  status: string;
  updatedAt: string;
  activeTurn?: { turnId: string; startedAt: string | null } | null;
  pendingPermissions: PermissionFacts[];
  runtimeInfo?: { model?: string | null } | null;
  lastUsage?: {
    totalCostUsd?: number;
    contextWindowMaxTokens?: number;
    contextWindowUsedTokens?: number;
  };
  lastError?: string;
  requiresAttention?: boolean;
  attentionReason?: "finished" | "error" | "permission" | null;
  attentionTimestamp?: string | null;
  archivedAt?: string | null;
  internal?: boolean;
}

export interface UsageFacts {
  displayName: string;
  status: string;
  windows: Array<{
    label: string;
    summary?: boolean;
    usedPct?: number | null;
    remainingPct?: number | null;
    resetsAt?: string | null;
  }>;
}

export interface Entry {
  agent: AgentFacts;
  project: string | null;
  live?: Live | null;
}

export interface DigestInput {
  entries: Entry[];
  usage: UsageFacts[];
  /** agentId -> the `since` a board marked as read; the card stays off the boards until it moves on. */
  seen: Record<string, number>;
  /** Profile names and provider catalogs for model labels; absent, ids are cleaned up instead. */
  models?: ModelDirectory;
  focus: string | null;
  settings: Pick<Settings, "allowActions" | "replyPreview" | "timeZone">;
  host: string;
  /** The `paseo stop` CLI is available to this plugin. */
  canStop: boolean;
  now: number;
  /** Board settings made in Paseo, with their rev. */
  boardSettings?: BoardSettings;
  /** The boards' language, when known. */
  lang?: "zh" | "en";
  /** Where sessions started on a board go. */
  compose?: { workspace: string; profile?: string };
}

function minutes(clock: string): number {
  const [h, m] = clock.split(":").map(Number);
  return (h ?? 0) * 60 + (m ?? 0);
}

/** The part of the plugin settings every board applies; `rev` is the caller's counter. */
export function boardSettingsOf(
  s: Pick<Settings, "sound" | "ledBrightness" | "quietHours">,
  rev: number,
): BoardSettings {
  const quiet = s.quietHours.enabled && s.quietHours.start !== s.quietHours.end;
  return {
    rev,
    sound: s.sound,
    ledBrightness: s.ledBrightness,
    quietStart: quiet ? minutes(s.quietHours.start) : 0,
    quietEnd: quiet ? minutes(s.quietHours.end) : 0,
  };
}

/** Read cards older than this leave the payload; the board retires them after an hour anyway. */
export const SEEN_KEEP_MS = 60 * 60 * 1000;
export const MAX_AGENTS = 24;
export const MAX_WINDOWS = 24;
const MAX_OPTIONS = 6;

const PERMISSION_OPTIONS = ["允许", "拒绝"];
const PLAN_OPTIONS = ["批准计划", "继续规划"];

/** One question of a question request (Claude AskUserQuestion shape, shared by Paseo's UI). */
export interface QuestionFacts {
  question: string;
  header?: string;
  options: Array<{ label: string }>;
}

export function questionsOf(request: PermissionFacts): QuestionFacts[] {
  const raw = request.input?.questions;
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((q): QuestionFacts[] => {
    if (!q || typeof q !== "object") return [];
    const rec = q as Record<string, unknown>;
    const question = typeof rec.question === "string" ? rec.question : "";
    const options = Array.isArray(rec.options)
      ? rec.options.flatMap((o) =>
          o && typeof o === "object" && typeof (o as { label?: unknown }).label === "string"
            ? [{ label: (o as { label: string }).label }]
            : [],
        )
      : [];
    if (!question && !options.length) return [];
    const header = typeof rec.header === "string" ? rec.header : undefined;
    return [{ question, header, options }];
  });
}

export function askKind(request: PermissionFacts): Ask["kind"] {
  if (request.kind === "question") return "question";
  if (request.kind === "plan") return "plan";
  return "permission";
}

/** The option labels the board shows, in the order `choice` indexes them. */
export function askOptions(request: PermissionFacts): string[] {
  const kind = askKind(request);
  if (kind === "question") {
    const first = questionsOf(request)[0];
    if (first?.options.length) return first.options.map((o) => o.label);
  }
  if (request.actions?.length) return request.actions.map((a) => a.label);
  if (kind === "plan") return PLAN_OPTIONS;
  if (kind === "question") return [];
  return PERMISSION_OPTIONS;
}

function askText(request: PermissionFacts): string {
  const kind = askKind(request);
  if (kind === "question") {
    const first = questionsOf(request)[0];
    if (first?.question) return first.question;
  }
  if (kind === "plan" && request.detail?.type === "plan" && typeof request.detail.text === "string") {
    return request.detail.text;
  }
  const detail = request.detail;
  if (detail?.type === "shell" && typeof detail.command === "string") {
    return `${request.title ?? request.name}: ${detail.command}`;
  }
  if (detail && typeof detail.filePath === "string") {
    return `${request.title ?? request.name}: ${detail.filePath}`;
  }
  return request.description || request.title || request.name;
}

export function toAsk(request: PermissionFacts): Ask {
  return {
    kind: askKind(request),
    requestId: request.id,
    text: clip(askText(request), 400),
    options: askOptions(request)
      .slice(0, MAX_OPTIONS)
      .map((o) => clip(o, 60)),
  };
}

export function stateOf(agent: AgentFacts): State {
  if (agent.pendingPermissions.length > 0) return "call";
  if (agent.status === "running" || (agent.status === "initializing" && agent.activeTurn)) {
    return "work";
  }
  if (agent.status === "error") return "fail";
  if (agent.requiresAttention && agent.attentionReason === "error") return "fail";
  if (agent.requiresAttention && agent.attentionReason === "finished") return "done";
  return "seen";
}

/** When the current turn started (work) or the last one ended (everything else). */
export function sinceOf(agent: AgentFacts, state: State): number | null {
  if (state === "work") {
    return epochMs(agent.activeTurn?.startedAt) ?? epochMs(agent.updatedAt);
  }
  return epochMs(agent.attentionTimestamp) ?? epochMs(agent.updatedAt);
}

function ctxOf(agent: AgentFacts): number | undefined {
  const u = agent.lastUsage;
  if (!u?.contextWindowMaxTokens || u.contextWindowUsedTokens == null) return undefined;
  const pct = Math.round((u.contextWindowUsedTokens / u.contextWindowMaxTokens) * 100);
  return Math.max(0, Math.min(100, pct));
}

/**
 * A board marked this card read and nothing happened since: no new turn, no new
 * attention. Waiting and working cards always show.
 */
export function markedRead(agent: AgentFacts, seen: Record<string, number>): boolean {
  const mark = seen[agent.id];
  if (mark == null) return false;
  const state = stateOf(agent);
  if (state === "call" || state === "work") return false;
  const since = sinceOf(agent, state);
  return since != null && since <= mark;
}

function digestAgent(entry: Entry, input: DigestInput, focused: boolean): Agent {
  const a = entry.agent;
  const state = stateOf(a);
  const since = sinceOf(a, state);
  const out: Agent = { id: a.id, state, since };
  const title = clip(a.title, 80);
  if (title) out.title = title;
  if (entry.project) out.project = clip(entry.project, 40);
  const model = modelLabelFor(input.models, a.provider, [a.runtimeInfo?.model, a.model]);
  if (model) out.model = model;
  const live = entry.live;

  if (state === "call") out.ask = toAsk(a.pendingPermissions[0]!);
  if (state === "work") {
    const actions = live?.actions ?? [];
    const latest = actions[actions.length - 1];
    if (latest) out.now = latest.summary;
    if (focused) {
      const recent = actions
        .slice(0, -1)
        .reverse()
        .slice(0, 3)
        .map((x) => x.summary);
      if (recent.length) out.recent = recent;
      if (live?.todo && live.todo.total > 0) out.todo = live.todo;
    }
    out.stoppable = input.canStop;
    if (a.activeTurn?.turnId) out.turn = a.activeTurn.turnId;
  }
  const ctx = ctxOf(a);
  if (ctx != null) out.ctx = ctx;
  if (state !== "work" && state !== "call") {
    const cost = formatCost(a.lastUsage?.totalCostUsd);
    if (cost) out.cost = cost;
  }
  if (state === "fail") {
    const error = clip(a.lastError, 160);
    if (error) out.error = error;
  }
  if ((state === "done" || state === "fail" || state === "seen") && input.settings.replyPreview) {
    const body = farolMarkdown(live?.reply, BODY_MAX);
    if (body) out.body = body;
  }
  return out;
}

const KEEP_RANK: Record<State, number> = { call: 0, work: 1, fail: 2, done: 3, seen: 4 };

function stateRank(s: State): number {
  return STATES.indexOf(s);
}

/** "Claude (someone@example.com)" -> "Claude": the provider without its account. */
export function usageGroup(displayName: string): string {
  return displayName.replace(/\s*\([^()]*\)\s*$/, "").trim() || displayName.trim();
}

export function digestUsage(usage: UsageFacts[]): { usage: Window[]; fuel?: Fuel } {
  const windows: Array<Window & { summary: boolean }> = [];
  for (const provider of usage) {
    if (provider.status !== "available") continue;
    const group = clip(usageGroup(provider.displayName), 32);
    for (const w of provider.windows) {
      const used =
        w.usedPct != null ? w.usedPct : w.remainingPct != null ? 100 - w.remainingPct : null;
      if (used == null || !Number.isFinite(used)) continue;
      const name = clip(w.label, 32);
      windows.push({
        label: clip(`${group} · ${name}`, 32),
        group,
        name,
        used: Math.max(0, Math.min(100, Math.round(used))),
        resets: epochMs(w.resetsAt),
        summary: w.summary === true,
      });
    }
  }
  const pool = windows.some((w) => w.summary) ? windows.filter((w) => w.summary) : windows;
  const tightest = pool.reduce<(typeof windows)[number] | undefined>(
    (best, w) => (!best || w.used > best.used ? w : best),
    undefined,
  );
  return {
    usage: windows.slice(0, MAX_WINDOWS).map(({ summary: _summary, ...w }) => w),
    ...(tightest ? { fuel: { label: tightest.label, left: 100 - tightest.used } } : {}),
  };
}

export function digest(input: DigestInput): Omit<Snapshot, "rev" | "idle"> {
  const agents: Agent[] = [];
  for (const entry of input.entries) {
    const a = entry.agent;
    if (a.archivedAt || a.internal || markedRead(a, input.seen)) continue;
    const focused = input.focus === a.id;
    const agent = digestAgent(entry, input, focused);
    const stale =
      agent.state === "seen" && (agent.since == null || input.now - agent.since >= SEEN_KEEP_MS);
    if (stale && !focused) continue;
    agents.push(agent);
  }
  const bySince = (x: Agent, y: Agent) => (y.since ?? 0) - (x.since ?? 0) || x.id.localeCompare(y.id);
  if (agents.length > MAX_AGENTS) {
    // What survives the cap: waiting and working cards first, so a pile of
    // unread finished cards can never hide a live one; then failed, finished, read.
    const keep = [...agents]
      .sort((x, y) => KEEP_RANK[x.state] - KEEP_RANK[y.state] || bySince(x, y))
      .slice(0, MAX_AGENTS);
    const focused = agents.find((a) => a.id === input.focus);
    if (focused && !keep.includes(focused)) keep[keep.length - 1] = focused;
    agents.length = 0;
    agents.push(...keep);
  }
  agents.sort((x, y) => stateRank(x.state) - stateRank(y.state) || bySince(x, y));
  return {
    v: PROTOCOL_VERSION,
    now: input.now,
    tz: tzOffsetMinutes(input.settings.timeZone, input.now),
    host: input.host,
    readOnly: !input.settings.allowActions,
    ...(input.boardSettings ? { settings: input.boardSettings } : {}),
    agents,
    ...digestUsage(input.usage),
    ...(input.lang ? { lang: input.lang } : {}),
    ...(input.compose ? { compose: input.compose } : {}),
  };
}
