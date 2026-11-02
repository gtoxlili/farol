import type { PluginClientContext } from "@getpaseo/plugin/client";
import { SettingsScreen } from "./client/settings";
import { FarolRow } from "./client/timeline-row";
import { languageRpc, TIMELINE_KIND, TimelineRowSchema } from "./shared/settings";

/** How often the app repeats its language to the plugin (it may restart). */
const LANGUAGE_EVERY_MS = 60_000;

export default function contribute(client: PluginClientContext) {
  client.addSettingsScreen({ id: "farol", title: "Farol", icon: "Lamp", Component: SettingsScreen });
  client.addTimelineRenderer({
    kind: TIMELINE_KIND,
    version: 1,
    schema: TimelineRowSchema,
    Component: FarolRow,
  });
  // Boards follow the Paseo app's language; only the app knows it. Repeated,
  // so a restarted plugin or a changed language is picked up within a minute.
  const report = () => void client.rpc(languageRpc, { locale: client.i18n.locale }).catch(() => undefined);
  report();
  const timer = setInterval(report, LANGUAGE_EVERY_MS);
  return () => clearInterval(timer);
}
