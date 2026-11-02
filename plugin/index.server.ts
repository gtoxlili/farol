import type { PluginServerContext } from "@getpaseo/plugin/server";
import { actRpc, focusRpc, nextRpc } from "./shared/protocol";
import { connectCodeRpc, farolSettings, statusRpc } from "./shared/settings";
import { Farol } from "./server/farol";
import { connectPayload, normalizeAddress, qrRows } from "./server/qr";

export default function contribute(server: PluginServerContext) {
  const farol = new Farol();

  const settings = server.registerSettings(farolSettings);
  const applySettings = (state: Awaited<ReturnType<typeof settings.read>>) => {
    if (state.status === "ready") farol.setSettings(state.values);
  };
  void settings.read().then(applySettings);
  const stopSettings = settings.subscribe(applySettings);

  // Board RPCs (farol/1).
  server.handle(nextRpc, (input, { paseo }) =>
    farol.next(paseo, input.device, input.cursor, input.focus, input.prefs),
  );
  server.handle(focusRpc, async (input, { paseo }) => {
    await farol.focus(paseo, input.device, input.focus ?? null, input.prefs);
    return { ok: true };
  });
  // farol/1 `Act` carries no device id, so timeline rows say "Farol" without naming the board.
  server.handle(actRpc, (input, { paseo }) => farol.act(paseo, input, ""));

  // Settings page RPCs.
  server.handle(statusRpc, async (_input, { paseo }) => {
    await farol.ensure(paseo).catch(() => undefined);
    return {
      host: farol.hostLabel,
      boards: farol.stream.boards(),
      canStop: farol.canStop,
      boardSound: farol.pendingBoardSound,
    };
  });
  server.handle(connectCodeRpc, ({ address, password, name }) => {
    const url = normalizeAddress(address);
    if (!url) return { ok: false as const, error: "地址无效：应为 wss://… 或 https://…" };
    const payload = connectPayload(url, password, name ?? farol.hostLabel);
    return { ok: true as const, url, payload, rows: qrRows(payload) };
  });

  // Notification-only hooks: nothing here changes an agent or a session.
  const offTurn = server.on("agent.turn_ended", ({ agent }, { paseo }) => farol.turnEnded(paseo, agent.id));
  const offAsk = server.on("agent.permission_requested", (_event, { paseo }) => {
    void farol.ensure(paseo).then(() => farol.stream.changed());
  });
  const offAnswer = server.on("agent.permission_resolved", (_event, { paseo }) => {
    void farol.ensure(paseo).then(() => farol.stream.changed());
  });

  return async () => {
    offTurn();
    offAsk();
    offAnswer();
    stopSettings();
    await farol.close();
  };
}
