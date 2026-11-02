import type { PluginTimelineItemProps } from "@getpaseo/plugin/client";
import { Text, View } from "react-native";
import type { TimelineRow } from "../shared/settings";

/** "在 farol-3a1f 上回答：光环 · 底部积光" / "在 farol-3a1f 上停止了这一轮" in the agent's timeline. */
export function FarolRow({ item, theme }: PluginTimelineItemProps<TimelineRow>) {
  const { action, text, device } = item.data;
  const where = device || "Farol";
  const line = action === "answer" ? `在 ${where} 上回答：${text}` : `在 ${where} 上停止了这一轮`;
  return (
    <View style={{ paddingVertical: 4 }}>
      <Text style={{ color: theme.colors.foregroundMuted, fontSize: 13 }}>{line}</Text>
    </View>
  );
}
