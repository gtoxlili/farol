// The workspace for sessions started on a board. Paseo's select menu does not
// scroll on phones and people keep dozens of workspaces, so this opens Paseo's
// modal with a search field and a scrolling list.
import type { PluginSurfaceProps } from "@getpaseo/plugin/client";
import { usePaseo, useTranslation } from "@getpaseo/plugin/client";
import { FlatList, Icon, Modal, SettingsAction, TextInput } from "@getpaseo/plugin/client/ui";
import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { Pressable, Text, View } from "react-native";
import { messages } from "./messages";

interface Choice {
  id: string;
  name: string;
  project: string;
}

/** Paseo lists at most this many workspaces per page. */
const PAGE = 200;

export function WorkspacePicker({
  theme,
  value,
  onChange,
}: Pick<PluginSurfaceProps, "theme"> & { value: string; onChange(id: string): void }) {
  const { t } = useTranslation(messages);
  const paseo = usePaseo();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const list = useQuery({
    queryKey: ["farol", "workspaces"],
    queryFn: async (): Promise<Choice[]> =>
      (await paseo.workspaces.list({ page: { limit: PAGE } })).entries.map((w) => ({
        id: w.id,
        name: w.name,
        project: w.projectDisplayName && w.projectDisplayName !== w.name ? w.projectDisplayName : "",
      })),
  });
  const current = list.data?.find((w) => w.id === value);
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    const all = list.data ?? [];
    const hits = q ? all.filter((w) => `${w.name} ${w.project}`.toLowerCase().includes(q)) : all;
    return q ? hits : [{ id: "", name: t("session.none"), project: "" }, ...hits];
  }, [list.data, query, t]);
  const close = () => {
    setOpen(false);
    setQuery("");
  };
  const c = theme.colors;
  return (
    <>
      <SettingsAction
        label={t("session.workspace")}
        hint={current ? [current.name, current.project].filter(Boolean).join(" · ") : value || t("session.none")}
        actionLabel={t("session.choose")}
        disabled={list.isLoading}
        onPress={() => setOpen(true)}
      />
      <Modal title={t("session.workspace")} open={open} onOpenChange={(next) => (next ? setOpen(true) : close())}>
        <Modal.Content scrollable={false} contentContainerStyle={{ padding: 0, gap: 0 }}>
          <View style={{ paddingHorizontal: 16, paddingTop: 4, paddingBottom: 8 }}>
            <TextInput
              value={query}
              onChangeText={setQuery}
              placeholder={t("session.search")}
              placeholderTextColor={c.foregroundMuted}
              autoCapitalize="none"
              autoCorrect={false}
              style={{
                color: c.foreground,
                backgroundColor: c.surface1,
                borderColor: c.border,
                borderWidth: 1,
                borderRadius: 10,
                paddingHorizontal: 12,
                paddingVertical: 10,
                fontSize: 15,
              }}
            />
          </View>
          <FlatList
            data={shown}
            keyExtractor={(w) => w.id || "none"}
            keyboardShouldPersistTaps="handled"
            style={{ flex: 1 }}
            ListEmptyComponent={
              <Text style={{ color: c.foregroundMuted, fontSize: 14, padding: 16 }}>{t("session.noMatch")}</Text>
            }
            renderItem={({ item }) => (
              <Pressable
                accessibilityRole="button"
                accessibilityState={{ selected: item.id === value }}
                onPress={() => {
                  onChange(item.id);
                  close();
                }}
                style={({ pressed }) => ({
                  flexDirection: "row",
                  alignItems: "center",
                  gap: 12,
                  paddingHorizontal: 16,
                  paddingVertical: 12,
                  backgroundColor: pressed ? c.surface2 : "transparent",
                })}
              >
                <View style={{ flex: 1, minWidth: 0 }}>
                  <Text numberOfLines={1} style={{ color: c.foreground, fontSize: 15 }}>
                    {item.name}
                  </Text>
                  {item.project ? (
                    <Text numberOfLines={1} style={{ color: c.foregroundMuted, fontSize: 13 }}>
                      {item.project}
                    </Text>
                  ) : null}
                </View>
                {item.id === value ? <Icon name="Check" size={18} color={c.accent} /> : null}
              </Pressable>
            )}
          />
        </Modal.Content>
      </Modal>
    </>
  );
}
