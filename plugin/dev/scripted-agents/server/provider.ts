// A development-only Paseo provider whose agents play scripts: tool calls,
// todos, questions, plans, permissions, failures, long turns. No model, no
// network, same result every time. Interrupts and permission answers reach the
// script exactly as they would a real provider.
import type {
  ProviderCapability,
  ProviderConnection,
  ProviderEvent,
  ProviderInput,
  ProviderPermissionResponse,
  ProviderRegistration,
} from "@getpaseo/plugin/server/provider";
import { scriptFor, type Step } from "./scripts";

const SUPPORTED: readonly ProviderCapability[] = ["prompt.message", "permission", "session.can_stop_runtime"];

interface Session {
  id: string;
  cwd: string;
  turn: Turn | null;
  turns: number;
}

interface Turn {
  id: string;
  canceled: boolean;
  timer: ReturnType<typeof setTimeout> | null;
  wake: (() => void) | null;
  permission: { id: string; resolve(r: ProviderPermissionResponse): void } | null;
}

/** Records every input the provider receives, for end-to-end checks. */
export const received: Array<{ at: number; type: string; sessionId?: string; detail?: unknown }> = [];

export function scriptedProvider(): ProviderRegistration {
  return {
    id: "scripted",
    label: "Scripted agents",
    description: "Development only: fake agents that play scripts. No model calls.",
    status: async () => ({ available: true }),
    connect: async (request) => connect(request.capabilities),
  };
}

function connect(offered: readonly string[]): ProviderConnection {
  const listeners = new Set<(event: ProviderEvent) => void>();
  const sessions = new Map<string, Session>();
  const capabilities = SUPPORTED.filter((c) => offered.includes(c));
  let seq = 0;
  const nextId = (prefix: string) => `${prefix}-${Date.now().toString(36)}-${++seq}`;
  const emit = (event: ProviderEvent) => {
    for (const l of listeners) l(event);
  };
  const item = (sessionId: string, value: Record<string, unknown>) =>
    emit({
      type: "timeline.item",
      sessionId,
      item: { id: nextId("item"), ...value } as never,
      timestamp: new Date().toISOString(),
    });

  const sleep = (turn: Turn, ms: number) =>
    new Promise<void>((resolve) => {
      turn.wake = resolve;
      turn.timer = setTimeout(() => {
        turn.timer = null;
        turn.wake = null;
        resolve();
      }, ms);
    });

  async function play(session: Session, turn: Turn, prompt: string) {
    const { steps } = scriptFor(prompt);
    let answer = "";
    let failed: string | null = null;
    for (const step of steps as Step[]) {
      if (turn.canceled) break;
      switch (step.kind) {
        case "say":
          item(session.id, { type: "assistant_message", text: step.text.replace("{answer}", answer) });
          break;
        case "todo":
          item(session.id, {
            type: "todo",
            items: step.items.map((i) => ({ ...i, completed: i.status === "completed" })),
          });
          break;
        case "tool": {
          const callId = nextId("call");
          const base = { type: "tool_call", callId, name: step.name, detail: step.detail };
          item(session.id, { ...base, status: "running", error: null });
          await sleep(turn, step.ms);
          item(session.id, { ...base, status: turn.canceled ? "canceled" : "completed", error: null });
          break;
        }
        case "wait":
          await sleep(turn, step.ms);
          break;
        case "ask": {
          const id = nextId("perm");
          const response = await new Promise<ProviderPermissionResponse>((resolve) => {
            turn.permission = { id, resolve };
            emit({ type: "session.permission", sessionId: session.id, request: { id, ...step.request } as never });
          });
          turn.permission = null;
          emit({ type: "session.permission_resolved", sessionId: session.id, permissionId: id });
          if (turn.canceled) break;
          answer = describe(response);
          break;
        }
        case "usage":
          emit({
            type: "session.usage",
            sessionId: session.id,
            turnId: turn.id,
            usage: { totalCostUsd: step.cost, contextWindowUsedTokens: step.used, contextWindowMaxTokens: step.max },
          });
          break;
        case "fail":
          failed = step.message;
          break;
      }
      if (failed) break;
    }
    session.turn = null;
    if (turn.canceled) {
      emit({ type: "session.turn", sessionId: session.id, turnId: turn.id, state: "canceled" });
    } else if (failed) {
      emit({ type: "session.turn", sessionId: session.id, turnId: turn.id, state: "failed", error: { message: failed } });
    } else {
      emit({ type: "session.turn", sessionId: session.id, turnId: turn.id, state: "completed" });
    }
  }

  async function send(input: ProviderInput): Promise<void> {
    received.push({ at: Date.now(), type: input.type, sessionId: "sessionId" in input ? input.sessionId : undefined });
    switch (input.type) {
      case "catalog":
        queueMicrotask(() =>
          emit({
            type: "catalog",
            requestId: input.requestId,
            catalog: {
              models: [{ id: "script", label: "Script", isDefault: true, contextWindowMaxTokens: 200000 }],
              modes: [{ id: "default", label: "Default" }],
              defaultModel: "script",
              defaultMode: "default",
            },
          }),
        );
        return;
      case "sessions":
        queueMicrotask(() => emit({ type: "sessions", requestId: input.requestId, sessions: [] }));
        return;
      case "session.open": {
        const session: Session = { id: input.sessionId, cwd: input.config.cwd, turn: null, turns: 0 };
        sessions.set(session.id, session);
        queueMicrotask(() => {
          emit({
            type: "session.opened",
            requestId: input.requestId,
            sessionId: session.id,
            capabilities,
            restoration: "core",
            cwd: session.cwd,
            ...(input.config.title ? { title: input.config.title } : {}),
          });
          emit({
            type: "session.config",
            sessionId: session.id,
            config: {
              model: "script",
              mode: "default",
              models: [{ id: "script", label: "Script", isDefault: true }],
              modes: [{ id: "default", label: "Default" }],
              thinkingOptions: [],
              settings: [],
            },
          });
          emit({ type: "session.ready", requestId: input.requestId, sessionId: session.id });
        });
        return;
      }
      case "session.prompt": {
        const session = sessions.get(input.sessionId);
        if (!session) throw new Error(`unknown session ${input.sessionId}`);
        const prompt = input.prompt;
        const text =
          prompt.input.type === "message"
            ? prompt.input.content.map((c) => (c.type === "text" ? c.text : "")).join(" ")
            : prompt.input.name;
        if (session.turn) {
          queueMicrotask(() =>
            emit({
              type: "session.prompt_result",
              sessionId: session.id,
              clientMessageId: prompt.clientMessageId,
              result: { type: "failed", error: { message: "a turn is already running" } },
            }),
          );
          return;
        }
        const turn: Turn = {
          id: `turn-${session.id.slice(0, 6)}-${++session.turns}`,
          canceled: false,
          timer: null,
          wake: null,
          permission: null,
        };
        session.turn = turn;
        queueMicrotask(() => {
          item(session.id, { type: "user_message", text, clientMessageId: prompt.clientMessageId });
          emit({
            type: "session.prompt_result",
            sessionId: session.id,
            clientMessageId: prompt.clientMessageId,
            result: { type: "turn", turnId: turn.id },
          });
          emit({ type: "session.turn", sessionId: session.id, turnId: turn.id, state: "started" });
          void play(session, turn, text);
        });
        return;
      }
      case "session.permission": {
        received[received.length - 1]!.detail = input.response;
        const turn = sessions.get(input.sessionId)?.turn;
        if (turn?.permission?.id === input.permissionId) turn.permission.resolve(input.response);
        return;
      }
      case "session.interrupt": {
        const turn = sessions.get(input.sessionId)?.turn;
        if (turn) {
          turn.canceled = true;
          if (turn.timer) clearTimeout(turn.timer);
          turn.timer = null;
          turn.wake?.();
          turn.wake = null;
          turn.permission?.resolve({ behavior: "deny", message: "interrupted" });
        }
        queueMicrotask(() => emit({ type: "request.completed", requestId: input.requestId }));
        return;
      }
      case "session.can_stop_runtime":
        queueMicrotask(() =>
          emit({ type: "session.can_stop_runtime", requestId: input.requestId, sessionId: input.sessionId, canStop: true }),
        );
        return;
      case "session.close": {
        const session = sessions.get(input.sessionId);
        if (session?.turn) {
          session.turn.canceled = true;
          if (session.turn.timer) clearTimeout(session.turn.timer);
          session.turn.wake?.();
        }
        sessions.delete(input.sessionId);
        queueMicrotask(() => {
          emit({ type: "request.completed", requestId: input.requestId });
          emit({ type: "session.closed", sessionId: input.sessionId });
        });
        return;
      }
      default:
        if ("requestId" in input) {
          const requestId = input.requestId;
          queueMicrotask(() =>
            emit({ type: "request.failed", requestId, error: { message: `unsupported: ${input.type}` } }),
          );
        }
    }
  }

  return {
    version: 1,
    capabilities,
    send,
    onEvent(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async close() {
      for (const s of sessions.values()) {
        if (s.turn) {
          s.turn.canceled = true;
          if (s.turn.timer) clearTimeout(s.turn.timer);
        }
      }
      sessions.clear();
      listeners.clear();
    },
  };
}

function describe(response: ProviderPermissionResponse): string {
  if (response.behavior === "deny") return `denied${response.message ? ` (${response.message})` : ""}`;
  const answers = response.updatedInput?.answers;
  if (answers && typeof answers === "object") {
    const first = Object.values(answers as Record<string, unknown>)[0];
    if (typeof first === "string") return first;
  }
  return response.selectedActionId ? `allowed (${response.selectedActionId})` : "allowed";
}
