// The exact frames rust/logic/src/paseo.rs and model.rs produce, through the
// plugin's input schemas.
import { describe, expect, it } from "vitest";
import { ActSchema, FocusSchema, NextSchema, SnapshotSchema } from "../shared/protocol";

describe("board frames", () => {
  it("accepts farol.next as paseo.rs sends it, with and without focus", () => {
    expect(NextSchema.parse({ v: 1, cursor: 0, device: "3a1f" })).toEqual({ v: 1, cursor: 0, device: "3a1f" });
    expect(NextSchema.parse({ v: 1, cursor: 7, focus: "a-work", device: "3a1f" }).focus).toBe("a-work");
  });

  it("accepts farol.focus, including clearing the focus", () => {
    expect(FocusSchema.parse({ focus: "a-work", device: "3a1f" }).focus).toBe("a-work");
    expect(FocusSchema.parse({ device: "3a1f" }).focus).toBeUndefined();
  });

  it("accepts fw and screen from firmware 1.0.0 and their absence from older boards", () => {
    const now = NextSchema.parse({ v: 1, cursor: 7, device: "3a1f", screen: "usage", fw: "1.0.0" });
    expect([now.screen, now.fw]).toEqual(["usage", "1.0.0"]);
    expect(FocusSchema.parse({ focus: "a", device: "3a1f", screen: "deck" }).screen).toBe("deck");
    const old = NextSchema.parse({ v: 1, cursor: 7, device: "3a1f" });
    expect([old.screen, old.fw]).toEqual([undefined, undefined]);
  });

  it("accepts model.rs Act serializations, with and without device", () => {
    expect(
      ActSchema.parse(JSON.parse(`{"agent":"a","device":"farol-3a1f","kind":"answer","requestId":"r","choice":1}`))
        .device,
    ).toBe("farol-3a1f");

    expect(ActSchema.parse(JSON.parse(`{"agent":"a","kind":"answer","requestId":"r","choice":1}`))).toEqual({
      agent: "a",
      kind: "answer",
      requestId: "r",
      choice: 1,
    });
    expect(ActSchema.parse({ agent: "a", kind: "stop" }).kind).toBe("stop");
    expect(ActSchema.parse({ agent: "a", kind: "seen" }).kind).toBe("seen");
  });

  it("idle answers are valid snapshots", () => {
    const idle = { v: 1, rev: 3, now: 5, tz: 0, host: "", readOnly: false, idle: true, agents: [], usage: [] };
    expect(SnapshotSchema.parse(idle)).toEqual(idle);
  });
});
