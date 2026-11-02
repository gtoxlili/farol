// Board actions -> Paseo calls, with the policy checks the board relies on:
// read-only mode, "is the request still open", at-most-once per request, an
// answer reported only once Paseo confirms it was the one applied, and a stop
// only for the turn the board showed.
import type { Act, ActResult } from "../shared/protocol";
import type { TimelineRow } from "../shared/settings";
import { type AgentFacts, askOptions, type PermissionFacts, questionsOf, sinceOf, stateOf } from "./digest";
import { clip } from "./text";

/** Paseo's AgentPermissionResponse, as far as Farol uses it. */
export type PermissionResponse =
  | { behavior: "allow"; selectedActionId?: string; updatedInput?: Record<string, unknown> }
  | { behavior: "deny"; selectedActionId?: string; message?: string };

/** How long an answer waits for Paseo to report which response was applied. */
export const RESOLUTION_TIMEOUT_MS = 3_000;

export interface ActionPorts {
  agent(id: string): AgentFacts | null;
  /** The agent as the daemon has it now (not the observer's copy). */
  fresh(id: string): Promise<AgentFacts | null>;
  allowActions(): boolean;
  canStop(): boolean;
  respond(agentId: string, requestId: string, response: PermissionResponse): Promise<void>;
  /** The response Paseo applied to `requestId` (agent.permission_resolved), or null after `timeoutMs`. */
  resolution(requestId: string, timeoutMs: number): Promise<PermissionResponse | null>;
  stop(agentId: string): Promise<void>;
  appendRow(agentId: string, rowId: string, row: TimelineRow): Promise<void>;
  markSeen(agentId: string, since: number): void;
  log: { warn(...args: unknown[]): void };
}

const fail = (reason: NonNullable<ActResult["reason"]>, message?: string): ActResult =>
  message ? { ok: false, reason, message: clip(message, 120) } : { ok: false, reason };

/** Whether Paseo applied this response (the parts a board answer sets). */
export function sameResponse(applied: PermissionResponse, sent: PermissionResponse): boolean {
  if (applied.behavior !== sent.behavior) return false;
  if ((applied.selectedActionId ?? null) !== (sent.selectedActionId ?? null)) return false;
  const answers = (r: PermissionResponse) =>
    r.behavior === "allow" ? JSON.stringify(r.updatedInput?.answers ?? null) : "null";
  return answers(applied) === answers(sent);
}

/** Maps option `choice` of a pending request to Paseo's response and the label shown. */
export function responseFor(
  request: PermissionFacts,
  choice: number,
): { response: PermissionResponse; label: string } | null {
  const options = askOptions(request);
  const label = options[choice];
  if (label === undefined) return null;
  if (request.kind === "question") {
    const first = questionsOf(request)[0];
    if (first?.options.length) {
      // Paseo's own question UI keys answers by header; Claude also accepts the
      // full question text. Both keys carry the same answer.
      const answers: Record<string, string> = {};
      if (first.header) answers[first.header] = label;
      if (first.question) answers[first.question] = label;
      return { response: { behavior: "allow", updatedInput: { answers } }, label };
    }
  }
  const action = request.actions?.[choice];
  if (action) {
    return { response: { behavior: action.behavior, selectedActionId: action.id }, label };
  }
  if (choice === 0) return { response: { behavior: "allow" }, label };
  const message = request.kind === "plan" ? "Keep planning." : "Denied from Farol.";
  return { response: { behavior: "deny", message }, label };
}

export class Actions {
  /** Requests answered (or being answered) from a board: at most once each. */
  private readonly answered = new Set<string>();
  private readonly stopping = new Set<string>();

  constructor(private readonly ports: ActionPorts) {}

  async act(input: Act, device: string): Promise<ActResult> {
    const agent = this.ports.agent(input.agent);
    if (!agent || agent.archivedAt) return fail("not_found");
    switch (input.kind) {
      case "seen":
        return this.seen(agent);
      case "answer":
        if (!this.ports.allowActions()) return fail("read_only");
        return this.answer(agent, input, device);
      case "stop":
        if (!this.ports.allowActions()) return fail("read_only");
        return this.stop(agent, device, input.turn);
    }
  }

  private seen(agent: AgentFacts): ActResult {
    const since = sinceOf(agent, stateOf(agent));
    if (since != null) this.ports.markSeen(agent.id, since);
    return { ok: true };
  }

  private async answer(agent: AgentFacts, input: Act, device: string): Promise<ActResult> {
    const requestId = input.requestId;
    if (!requestId) return fail("failed", "missing requestId");
    const request = agent.pendingPermissions.find((p) => p.id === requestId);
    if (!request || this.answered.has(requestId)) return fail("already_resolved");
    const mapped = responseFor(request, input.choice ?? 0);
    if (!mapped) return fail("failed", "no such option");
    this.answered.add(requestId);
    // Listen before sending: the resolution can arrive before respond() returns.
    const applied = this.ports.resolution(requestId, RESOLUTION_TIMEOUT_MS);
    try {
      await this.ports.respond(agent.id, requestId, mapped.response);
    } catch (error) {
      this.answered.delete(requestId);
      return fail("failed", error instanceof Error ? error.message : String(error));
    }
    // Paseo applies one response per request; another client may have won.
    const resolution = await applied;
    if (resolution === null) {
      const now = await this.ports.fresh(agent.id);
      if (now?.pendingPermissions.some((p) => p.id === requestId)) {
        this.answered.delete(requestId);
        return fail("failed", "Paseo did not confirm the answer");
      }
      // Resolved without a report in time: most likely ours, but not provably.
    } else if (!sameResponse(resolution, mapped.response)) {
      return fail("already_resolved");
    }
    void this.ports
      .appendRow(agent.id, `farol-${requestId}`, { action: "answer", text: mapped.label, device })
      .catch((error) => this.ports.log.warn("farol: timeline append failed", String(error)));
    return { ok: true };
  }

  /**
   * Stops the turn the board showed. The CLI cancels whatever turn is current,
   * so the daemon's own copy is read just before and must still be that turn;
   * a turn started elsewhere in the last moments is left alone. Boards that
   * name no turn (older firmware) stop the current one.
   */
  private async stop(agent: AgentFacts, device: string, turn?: string): Promise<ActResult> {
    if (!this.ports.canStop()) return fail("failed", "paseo CLI not found");
    if (this.stopping.has(agent.id)) return { ok: true };
    const now = (await this.ports.fresh(agent.id)) ?? agent;
    if (stateOf(now) !== "work") return fail("already_resolved");
    if (turn && now.activeTurn?.turnId !== turn) return fail("already_resolved");
    this.stopping.add(agent.id);
    try {
      await this.ports.stop(agent.id);
    } catch (error) {
      return fail("failed", error instanceof Error ? error.message : String(error));
    } finally {
      this.stopping.delete(agent.id);
    }
    const stopped = now.activeTurn?.turnId ?? String(Date.now());
    void this.ports
      .appendRow(agent.id, `farol-stop-${stopped}`, { action: "stop", text: "", device })
      .catch((error) => this.ports.log.warn("farol: timeline append failed", String(error)));
    return { ok: true };
  }
}
