// The exact frames rust/logic/src/paseo.rs and model.rs produce, through the
// plugin's input schemas; and the connection code through a decoder that
// mirrors rust/logic/src/qr.rs.
import { describe, expect, it } from "vitest";
import { ActSchema, FocusSchema, NextSchema, SnapshotSchema } from "../shared/protocol";
import { connectPayload, normalizeAddress, qrRows } from "../server/qr";

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

/** rust/logic/src/qr.rs `farol()` + `percent_decode()`, in TypeScript. */
function parseFarol(payload: string): { url: string; password: string; host: string } | null {
  const prefix = "farol://connect?";
  if (!payload.startsWith(prefix)) return null;
  let url: string | null = null;
  let password = "";
  let host = "";
  for (const pair of payload.slice(prefix.length).split("&")) {
    const at = pair.indexOf("=");
    if (at < 0) return null;
    const value = decodeURIComponent(pair.slice(at + 1).replace(/\+/g, " "));
    const key = pair.slice(0, at);
    if (key === "u") url = value;
    else if (key === "p") password = value;
    else if (key === "n") host = value;
  }
  if (!url || !(url.startsWith("ws://") || url.startsWith("wss://"))) return null;
  return { url, password, host };
}

describe("connection code", () => {
  it("normalizes what people paste into the daemon's WebSocket URL", () => {
    expect(normalizeAddress("paseo.example.net")).toBe("wss://paseo.example.net/ws");
    expect(normalizeAddress("https://paseo.example.net")).toBe("wss://paseo.example.net/ws");
    expect(normalizeAddress("http://192.168.1.20:6767")).toBe("ws://192.168.1.20:6767/ws");
    expect(normalizeAddress("wss://paseo.example.net:8443/ws")).toBe("wss://paseo.example.net:8443/ws");
    expect(normalizeAddress("ftp://x")).toBeNull();
    expect(normalizeAddress("  ")).toBeNull();
  });

  it("round-trips through the board's parser, including awkward passwords", () => {
    for (const password of ["s&cret", "a+b c=d", "密码%20", ""]) {
      const payload = connectPayload("wss://paseo.example.net/ws", password, "Studio Mac");
      expect(parseFarol(payload)).toEqual({ url: "wss://paseo.example.net/ws", password, host: "Studio Mac" });
    }
  });

  it("matches the board's own test vector", () => {
    expect(parseFarol("farol://connect?u=wss%3A%2F%2Fpaseo.example.net%2Fws&p=s%26cret&n=Studio%20Mac")).toEqual(
      parseFarol(connectPayload("wss://paseo.example.net/ws", "s&cret", "Studio Mac")),
    );
  });

  it("draws a square QR with finder patterns", () => {
    const rows = qrRows(connectPayload("wss://paseo.example.net/ws", "pw", "Studio Mac"));
    const n = rows.length;
    expect(n).toBeGreaterThanOrEqual(21);
    expect(rows.every((r) => r.length === n)).toBe(true);
    expect(rows[0]!.slice(0, 7)).toBe("1111111");
    expect(rows[1]!.slice(0, 7)).toBe("1000001");
    expect(rows[0]!.slice(n - 7)).toBe("1111111");
  });
});
