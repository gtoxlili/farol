import { describe, expect, it, vi } from "vitest";
import { Actions, type ActionPorts, type PermissionResponse, responseFor } from "../server/actions";
import type { AgentFacts, PermissionFacts } from "../server/digest";

const question: PermissionFacts = {
  id: "q1",
  name: "AskUserQuestion",
  kind: "question",
  input: {
    questions: [
      { question: "Which style?", header: "Style", options: [{ label: "Halo" }, { label: "Plasma" }] },
    ],
  },
};

function agent(patch: Partial<AgentFacts> = {}): AgentFacts {
  return {
    id: "a",
    title: "t",
    provider: "claude",
    model: null,
    status: "idle",
    updatedAt: "2026-11-02T05:00:00.000Z",
    pendingPermissions: [],
    ...patch,
  };
}

/** Ports over one agent; by default Paseo applies whatever the board sent. */
function ports(current: AgentFacts | null, patch: Partial<ActionPorts> = {}): ActionPorts {
  let sent: PermissionResponse | null = null;
  return {
    agent: () => current,
    fresh: async () => current,
    allowActions: () => true,
    canStop: () => true,
    respond: vi.fn(async (_agent: string, _request: string, response: PermissionResponse) => {
      sent = response;
    }),
    resolution: vi.fn(async () => {
      await Promise.resolve();
      return sent;
    }),
    stop: vi.fn(async () => {}),
    appendRow: vi.fn(async () => {}),
    markSeen: vi.fn(),
    log: { warn: () => {} },
    ...patch,
  };
}

describe("responseFor", () => {
  it("answers questions keyed by header and question text", () => {
    expect(responseFor(question, 1)).toEqual({
      label: "Plasma",
      response: { behavior: "allow", updatedInput: { answers: { Style: "Plasma", "Which style?": "Plasma" } } },
    });
    expect(responseFor(question, 2)).toBeNull();
  });

  it("uses the request's own actions when it has them", () => {
    const plan: PermissionFacts = {
      id: "p",
      name: "ExitPlanMode",
      kind: "plan",
      actions: [
        { id: "implement", label: "Implement", behavior: "allow" },
        { id: "keep", label: "Keep planning", behavior: "deny" },
      ],
    };
    expect(responseFor(plan, 1)).toEqual({
      label: "Keep planning",
      response: { behavior: "deny", selectedActionId: "keep" },
    });
  });

  it("maps plain permissions to allow / deny", () => {
    const tool: PermissionFacts = { id: "t", name: "Bash", kind: "tool" };
    expect(responseFor(tool, 0)?.response).toEqual({ behavior: "allow" });
    expect(responseFor(tool, 1)?.response.behavior).toBe("deny");
  });
});

describe("Actions", () => {
  it("answers an open request once, then reports already_resolved", async () => {
    const p = ports(agent({ pendingPermissions: [question] }));
    const actions = new Actions(p);
    expect(await actions.act({ agent: "a", kind: "answer", requestId: "q1", choice: 0 }, "")).toEqual({ ok: true });
    expect(p.respond).toHaveBeenCalledTimes(1);
    expect(p.appendRow).toHaveBeenCalledWith("a", "farol-q1", { action: "answer", text: "Halo", device: "" });
    expect(await actions.act({ agent: "a", kind: "answer", requestId: "q1", choice: 1 }, "")).toEqual({
      ok: false,
      reason: "already_resolved",
    });
    expect(p.respond).toHaveBeenCalledTimes(1);
  });

  it("names the acting board in the timeline row", async () => {
    const p = ports(agent({ pendingPermissions: [question] }));
    await new Actions(p).act({ agent: "a", device: "farol-3a1f", kind: "answer", requestId: "q1", choice: 1 }, "farol-3a1f");
    expect(p.appendRow).toHaveBeenCalledWith("a", "farol-q1", { action: "answer", text: "Plasma", device: "farol-3a1f" });
  });

  it("reports already_resolved for a request that is no longer pending", async () => {
    const actions = new Actions(ports(agent()));
    expect((await actions.act({ agent: "a", kind: "answer", requestId: "gone", choice: 0 }, "")).reason).toBe(
      "already_resolved",
    );
  });

  it("refuses answers and stops in read-only mode but still records seen", async () => {
    const p = ports(agent({ pendingPermissions: [question], status: "running" }), { allowActions: () => false });
    const actions = new Actions(p);
    expect((await actions.act({ agent: "a", kind: "answer", requestId: "q1", choice: 0 }, "")).reason).toBe("read_only");
    expect((await actions.act({ agent: "a", kind: "stop" }, "")).reason).toBe("read_only");
    expect(p.respond).not.toHaveBeenCalled();
    expect(p.stop).not.toHaveBeenCalled();
  });

  it("reports not_found for unknown or archived agents", async () => {
    expect((await new Actions(ports(null)).act({ agent: "x", kind: "seen" }, "")).reason).toBe("not_found");
    const archived = agent({ archivedAt: "2026-11-02T05:00:00.000Z" });
    expect((await new Actions(ports(archived)).act({ agent: "a", kind: "seen" }, "")).reason).toBe("not_found");
  });

  it("stops only a running turn", async () => {
    const idle = ports(agent());
    expect((await new Actions(idle).act({ agent: "a", kind: "stop" }, "")).reason).toBe("already_resolved");
    const running = ports(agent({ status: "running", activeTurn: { turnId: "t1", startedAt: null } }));
    expect(await new Actions(running).act({ agent: "a", kind: "stop" }, "")).toEqual({ ok: true });
    expect(running.stop).toHaveBeenCalledWith("a");
    expect(running.appendRow).toHaveBeenCalledWith("a", "farol-stop-t1", { action: "stop", text: "", device: "" });
  });

  it("reports a failed stop with its message", async () => {
    const p = ports(agent({ status: "running" }), {
      stop: vi.fn(async () => {
        throw new Error("daemon unreachable");
      }),
    });
    expect(await new Actions(p).act({ agent: "a", kind: "stop" }, "")).toEqual({
      ok: false,
      reason: "failed",
      message: "daemon unreachable",
    });
    const noCli = ports(agent({ status: "running" }), { canStop: () => false });
    expect((await new Actions(noCli).act({ agent: "a", kind: "stop" }, "")).reason).toBe("failed");
  });

  it("marks seen at the turn's end time", async () => {
    const p = ports(
      agent({ requiresAttention: true, attentionReason: "finished", attentionTimestamp: "2026-11-02T05:35:00.000Z" }),
    );
    expect(await new Actions(p).act({ agent: "a", kind: "seen" }, "")).toEqual({ ok: true });
    expect(p.markSeen).toHaveBeenCalledWith("a", Date.parse("2026-11-02T05:35:00.000Z"));
  });

  it("reports an answer only when Paseo applied it, not when another client won", async () => {
    const phone = responseFor(question, 1)!.response;
    const p = ports(agent({ pendingPermissions: [question] }), { resolution: async () => phone });
    expect(await new Actions(p).act({ agent: "a", kind: "answer", requestId: "q1", choice: 0 }, "")).toEqual({
      ok: false,
      reason: "already_resolved",
    });
    expect(p.appendRow).not.toHaveBeenCalled();
  });

  it("fails an answer Paseo never confirmed while the request is still pending", async () => {
    const open = agent({ pendingPermissions: [question] });
    const p = ports(open, { resolution: async () => null });
    const actions = new Actions(p);
    expect((await actions.act({ agent: "a", kind: "answer", requestId: "q1", choice: 0 }, "")).reason).toBe("failed");
    expect(p.appendRow).not.toHaveBeenCalled();
    expect(p.respond).toHaveBeenCalledTimes(1);
    await actions.act({ agent: "a", kind: "answer", requestId: "q1", choice: 0 }, "");
    expect(p.respond).toHaveBeenCalledTimes(2);
  });

  it("stops only the turn the board showed", async () => {
    const shown = agent({ status: "running", activeTurn: { turnId: "t1", startedAt: null } });
    const now = agent({ status: "running", activeTurn: { turnId: "t2", startedAt: null } });
    const p = ports(shown, { fresh: async () => now });
    const actions = new Actions(p);
    expect((await actions.act({ agent: "a", kind: "stop", turn: "t1" }, "")).reason).toBe("already_resolved");
    expect(p.stop).not.toHaveBeenCalled();
    expect(await actions.act({ agent: "a", kind: "stop", turn: "t2" }, "")).toEqual({ ok: true });
    expect(p.stop).toHaveBeenCalledWith("a");
  });

  it("lets a failed respond be retried", async () => {
    let calls = 0;
    const p = ports(agent({ pendingPermissions: [question] }), {
      respond: vi.fn(async () => {
        calls += 1;
        if (calls === 1) throw new Error("socket closed");
      }),
      resolution: async () => responseFor(question, 0)!.response,
    });
    const actions = new Actions(p);
    expect((await actions.act({ agent: "a", kind: "answer", requestId: "q1", choice: 0 }, "")).reason).toBe("failed");
    expect((await actions.act({ agent: "a", kind: "answer", requestId: "q1", choice: 0 }, "")).ok).toBe(true);
  });
});
