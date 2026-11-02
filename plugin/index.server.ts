import type { PluginServerContext } from "@getpaseo/plugin/server";
import { actRpc, focusRpc, nextRpc, startRpc } from "./shared/protocol";
import { farolSettings, languageRpc, statusRpc } from "./shared/settings";
import type { PermissionResponse } from "./server/actions";
import { Farol } from "./server/farol";

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
    farol.next(paseo, input.device, input.cursor, input.focus, input.prefs, {
      fw: input.fw,
      screen: input.screen,
    }),
  );
  server.handle(focusRpc, async (input, { paseo }) => {
    await farol.focus(paseo, input.device, input.focus ?? null, input.prefs, input.screen);
    return { ok: true };
  });
  // `device` names the board in the timeline row; boards before firmware 1.0.0 omit it.
  server.handle(actRpc, (input, { paseo }) => farol.act(paseo, input, input.device ?? ""));
  server.handle(startRpc, (input, { paseo }) => farol.start(paseo, input));

  // The Paseo app's language (its client reports it; boards follow it on "auto").
  server.handle(languageRpc, ({ locale }) => {
    farol.setLocale(locale);
    return { ok: true as const };
  });

  // Settings page RPCs.
  server.handle(statusRpc, async (_input, { paseo }) => {
    await farol.ensure(paseo).catch(() => undefined);
    return {
      host: farol.hostLabel,
      boards: farol.stream.boards(),
      canStop: farol.canStop,
      usageAt: farol.usageAt,
      boardLanguage: farol.boardLanguage,
    };
  });

  // Notification-only hooks: nothing here changes an agent or a session.
  const offTurn = server.on("agent.turn_ended", ({ agent }, { paseo }) => farol.turnEnded(paseo, agent.id));
  const offAsk = server.on("agent.permission_requested", (_event, { paseo }) => {
    void farol.ensure(paseo).then(() => farol.stream.changed());
  });
  const offAnswer = server.on("agent.permission_resolved", ({ requestId, resolution }, { paseo }) => {
    farol.resolved(requestId, resolution as PermissionResponse);
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
