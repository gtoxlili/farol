import type { PluginTimelineItemProps } from "@getpaseo/plugin/client";
import { useTranslation } from "@getpaseo/plugin/client";
import { Text, View } from "react-native";
import type { TimelineRow } from "../shared/settings";
import { messages } from "./messages";

/** "Answered on farol-3a1f: …" / "Stopped this turn on farol-3a1f" in the agent's timeline. */
export function FarolRow({ item, theme }: PluginTimelineItemProps<TimelineRow>) {
  const { t } = useTranslation(messages);
  const { action, text, device } = item.data;
  const where = device || "Farol";
  const line = action === "answer" ? t("timeline.answer", { where, text }) : t("timeline.stop", { where });
  return (
    <View style={{ paddingVertical: 4 }}>
      <Text style={{ color: theme.colors.foregroundMuted, fontSize: 13 }}>{line}</Text>
    </View>
  );
}
