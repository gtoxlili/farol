import type { PluginClientContext } from "@getpaseo/plugin/client";
import { AddDeviceScreen, SettingsScreen } from "./client/settings";
import { FarolRow } from "./client/timeline-row";
import { TIMELINE_KIND, TimelineRowSchema } from "./shared/settings";

export default function contribute(client: PluginClientContext) {
  client.addSettingsScreen({ id: "farol", title: "Farol", icon: "Lamp", Component: SettingsScreen });
  client.addScreen({ id: "add-device", title: "Farol：添加设备", Component: AddDeviceScreen });
  client.addCommandCenterItem({
    id: "add-device",
    title: "Farol：添加设备",
    icon: "QrCode",
    context: "global",
    keywords: ["farol", "board", "lamp", "pair", "qr", "板子", "台灯", "连接码"],
    onSelect({ openScreen }) {
      openScreen({ screenId: "add-device" });
    },
  });
  client.addTimelineRenderer({
    kind: TIMELINE_KIND,
    version: 1,
    schema: TimelineRowSchema,
    Component: FarolRow,
  });
  return () => {};
}
