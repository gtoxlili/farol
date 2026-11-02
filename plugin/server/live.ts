// Folds an agent's timeline into what a card shows: the latest actions, todo
// progress and the last reply. Fed from timeline refetches and live streams.
import { clip, shortPath } from "./text";

/** The timeline item fields this module reads (a subset of Paseo's AgentTimelineItem). */
export type TimelineItem =
  | { type: "assistant_message"; text: string }
  | { type: "user_message"; text: string }
  | {
      type: "tool_call";
      callId: string;
      name: string;
      status: string;
      detail?: ToolDetail;
    }
  | {
      type: "todo";
      items: Array<{ text: string; completed: boolean; status?: string; activeForm?: string }>;
    }
  | { type: string; [key: string]: unknown };

type ToolDetail = { type: string; [key: string]: unknown };

export interface Action {
  callId: string;
  summary: string;
  running: boolean;
}

export interface Live {
  /** Oldest first, at most `KEEP_ACTIONS`. */
  actions: Action[];
  todo: { done: number; total: number; current: string } | null;
  reply: string | null;
  /** The previous item was assistant text, so a streamed chunk extends it. */
  replyOpen: boolean;
}

const KEEP_ACTIONS = 4;
const ACTION_CHARS = 96;
const REPLY_CHARS = 2000;

export function emptyLive(): Live {
  return { actions: [], todo: null, reply: null, replyOpen: false };
}

function str(v: unknown): string {
  return typeof v === "string" ? v : "";
}

/** One line for a tool call: the command for shells, "verb  target" otherwise. */
export function summarizeTool(name: string, detail: ToolDetail | undefined, cwd: string | null): string {
  const d = detail ?? { type: "unknown" };
  // Two spaces after the verb, as the board's fixtures and demo show actions.
  const line = (verb: string, target: string) => `${verb}  ${clip(target, ACTION_CHARS - verb.length - 2)}`;
  switch (d.type) {
    case "shell":
      return clip(str(d.command), ACTION_CHARS);
    case "read":
      return line("read", shortPath(str(d.filePath), cwd));
    case "edit":
      return line("edit", shortPath(str(d.filePath), cwd));
    case "write":
      return line("write", shortPath(str(d.filePath), cwd));
    case "search": {
      const tool = str(d.toolName) || "search";
      return line(tool === "web_search" ? "web" : tool, str(d.query));
    }
    case "fetch":
      return line("fetch", str(d.url));
    case "sub_agent":
      return line("agent", str(d.description) || str(d.subAgentType));
    case "plain_text":
      return clip(str(d.label) || str(d.text) || name, ACTION_CHARS);
    case "plan":
      return "plan";
    case "worktree_setup":
      return line("worktree", str(d.branchName));
    default:
      return clip(name, ACTION_CHARS);
  }
}

/** Applies one timeline item. `streamed` items may be chunks of a longer message. */
export function applyItem(live: Live, item: TimelineItem, cwd: string | null, streamed: boolean): Live {
  switch (item.type) {
    case "tool_call": {
      const call = item as Extract<TimelineItem, { type: "tool_call" }>;
      const action: Action = {
        callId: call.callId,
        summary: summarizeTool(call.name, call.detail, cwd),
        running: call.status === "running",
      };
      const rest = live.actions.filter((a) => a.callId !== call.callId);
      const at = live.actions.findIndex((a) => a.callId === call.callId);
      const actions =
        at >= 0
          ? live.actions.map((a) => (a.callId === call.callId ? action : a))
          : [...rest, action].slice(-KEEP_ACTIONS);
      return { ...live, actions, replyOpen: false };
    }
    case "todo": {
      const items = (item as Extract<TimelineItem, { type: "todo" }>).items ?? [];
      const done = items.filter((i) => i.completed || i.status === "completed").length;
      const active =
        items.find((i) => i.status === "in_progress") ??
        items.find((i) => !(i.completed || i.status === "completed"));
      const current = active ? clip(active.activeForm || active.text, ACTION_CHARS) : "";
      return { ...live, todo: { done, total: items.length, current }, replyOpen: false };
    }
    case "assistant_message": {
      const text = str((item as { text?: unknown }).text);
      // Paseo records a failed turn as "[System Error] …"; the card shows that as `error`.
      if (text.startsWith("[System Error]")) return { ...live, replyOpen: false };
      const reply = streamed && live.replyOpen ? (live.reply ?? "") + text : text;
      return { ...live, reply: reply.slice(-REPLY_CHARS), replyOpen: true };
    }
    case "user_message":
      // A new turn: the previous reply and plan no longer describe it.
      return { ...live, reply: null, replyOpen: false };
    default:
      return { ...live, replyOpen: false };
  }
}

export function foldItems(items: readonly TimelineItem[], cwd: string | null): Live {
  return items.reduce<Live>((live, item) => applyItem(live, item, cwd, false), emptyLive());
}
