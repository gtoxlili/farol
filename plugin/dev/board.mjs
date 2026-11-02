// A stand-in for the board's Paseo session (rust/logic/src/paseo.rs): one
// WebSocket, the same hello, then only plugin RPCs. Frames are built exactly
// as paseo.rs builds them. Node 22+ (global WebSocket).

export class FakeBoard {
  constructor({ url, password, device = "devboard", fw = "1.0.0", appVersion = "farol/1.0.0", log = () => {} }) {
    Object.assign(this, { url, password, device, fw, appVersion, log });
    this.screen = "home";
    this.nextId = 0;
    this.pending = new Map();
    this.cursor = 0;
    this.focus = null;
    this.prefs = null;
  }

  /** Opens the socket and says hello. Resolves with server_info, rejects on hello.rejected. */
  connect() {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(this.url);
      this.ws = ws;
      let ready = false;
      ws.addEventListener("open", () => {
        const hello = {
          type: "hello",
          clientId: `farol-${this.device}`,
          clientType: "cli",
          protocolVersion: 1,
          appVersion: this.appVersion,
          capabilities: { hello_rejection: true, owned_subscriptions: true },
        };
        if (this.password) hello.auth = { kind: "password", password: this.password };
        ws.send(JSON.stringify(hello));
      });
      ws.addEventListener("message", (ev) => {
        const frame = JSON.parse(String(ev.data));
        if (frame.type === "hello.rejected") {
          reject(Object.assign(new Error(`hello.rejected: ${frame.reason}`), { frame }));
          return;
        }
        if (frame.type === "ping") {
          ws.send(JSON.stringify({ type: "pong" }));
          return;
        }
        if (frame.type !== "session") return;
        const m = frame.message;
        if (m.type === "status" && m.payload?.status === "server_info" && !ready) {
          ready = true;
          this.serverInfo = m.payload;
          this.pinger = setInterval(() => ws.readyState === 1 && ws.send('{"type":"ping"}'), 10_000);
          resolve(m.payload);
        } else if (m.type === "plugin.rpc.invoke.response") {
          this.settle(m.payload.requestId, { output: m.payload.output });
        } else if (m.type === "rpc_error") {
          this.settle(m.payload.requestId, { error: m.payload.error });
        }
      });
      ws.addEventListener("close", (ev) => {
        clearInterval(this.pinger);
        for (const [, p] of this.pending) p({ error: `closed ${ev.code}` });
        this.pending.clear();
        if (!ready) reject(new Error(`closed before ready: ${ev.code} ${ev.reason}`));
      });
      ws.addEventListener("error", () => {});
    });
  }

  settle(id, result) {
    const p = this.pending.get(id);
    if (!p) return;
    this.pending.delete(id);
    p(result);
  }

  /** One plugin RPC, framed like paseo.rs `rpc()`. Resolves {output} or {error}. */
  rpc(method, input) {
    this.nextId += 1;
    const requestId = `f${this.nextId}`;
    const frame = {
      type: "session",
      message: { type: "plugin.rpc.invoke.request", requestId, pluginId: "farol", method, input },
    };
    const started = Date.now();
    return new Promise((resolve) => {
      this.pending.set(requestId, (r) => resolve({ ...r, ms: Date.now() - started }));
      this.ws.send(JSON.stringify(frame));
    });
  }

  /** `farol.next` as paseo.rs sends it (focus omitted when none). */
  next() {
    const input = { v: 1, cursor: this.cursor, device: this.device };
    if (this.focus) input.focus = this.focus;
    input.screen = this.screen;
    input.fw = this.fw;
    // Control-centre settings ride every poll until one is answered.
    const carried = this.prefs;
    if (carried) input.prefs = carried;
    return this.rpc("farol.next", input).then((r) => {
      if (r.output && r.output.v === 1) {
        this.cursor = r.output.rev;
        if (carried && this.prefs === carried) this.prefs = null;
      }
      return r;
    });
  }

  /** paseo.rs `set_focus(focus, screen)`: the deck when a card is on screen. */
  setFocus(focus, screen = focus ? "deck" : "home") {
    if (this.focus === focus && this.screen === screen) return null;
    this.focus = focus;
    this.screen = screen;
    return this.sendFocus();
  }

  /** Settings changed on the board's control centre (model.rs BoardPrefs). */
  setPrefs(prefs) {
    this.prefs = prefs;
    return this.sendFocus();
  }

  sendFocus() {
    const input = { device: this.device };
    if (this.focus) input.focus = this.focus;
    input.screen = this.screen;
    if (this.prefs) input.prefs = this.prefs;
    return this.rpc("farol.focus", input);
  }

  /** `farol.act`; fields named and omitted like model.rs `Act`. */
  act(agent, kind, requestId, choice) {
    const input = { agent, device: `farol-${this.device}`, kind };
    if (requestId != null) input.requestId = requestId;
    if (choice != null) input.choice = choice;
    return this.rpc("farol.act", input);
  }

  close() {
    clearInterval(this.pinger);
    this.ws?.close();
  }
}
