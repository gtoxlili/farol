import type { PluginClientContext } from "@getpaseo/plugin/client";
import { SettingsScreen } from "./client/settings";
import { FarolRow } from "./client/timeline-row";
import { TIMELINE_KIND, TimelineRowSchema } from "./shared/settings";

export default function contribute(client: PluginClientContext) {
  client.addSettingsScreen({ id: "farol", title: "Farol", icon: "Lamp", Component: SettingsScreen });
  client.addTimelineRenderer({
    kind: TIMELINE_KIND,
    version: 1,
    schema: TimelineRowSchema,
    Component: FarolRow,
  });
  return () => {};
}
