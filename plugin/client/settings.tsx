// Settings › Plugins › Farol: recent boards, where sessions started on a board
// go, and what boards may do. Boards connect to Paseo on their own (direct or
// relay), so nothing here pairs them. React Native primitives and host
// settings rows only.
import type { PluginSurfaceProps } from "@getpaseo/plugin/client";
import { usePaseo, useRpc, useSettings, useTranslation } from "@getpaseo/plugin/client";
import {
  ScrollView,
  SettingsAction,
  SettingsInput,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
  SettingsSwitch,
} from "@getpaseo/plugin/client/ui";
import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { Text } from "react-native";
import { farolSettings, type Settings, statusRpc } from "../shared/settings";
import { type Messages, messages } from "./messages";

type HostProps = Pick<PluginSurfaceProps, "theme" | "layout">;
type T = ReturnType<typeof useTranslation<Messages>>["t"];

function ago(t: T, ms: number, now: number): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return t("ago.now");
  if (s < 3600) return t("ago.minutes", { n: Math.round(s / 60) });
  if (s < 86400) return t("ago.hours", { n: Math.round(s / 3600) });
  return new Date(ms).toLocaleDateString();
}

function Boards({ theme }: HostProps) {
  const { t } = useTranslation(messages);
  const status = useRpc(statusRpc);
  const query = useQuery({
    queryKey: ["farol", "status"],
    queryFn: () => status({}),
    refetchInterval: 5000,
  });
  const muted = { color: theme.colors.foregroundMuted, fontSize: 13 };
  const now = Date.now();
  const boards = query.data?.boards ?? [];
  const onOff = (on: boolean) => (on ? t("boards.on") : t("boards.off"));
  return (
    <SettingsSection title={t("boards.title")}>
      {query.isError ? <Text style={muted}>{t("boards.unreadable", { error: String(query.error) })}</Text> : null}
      {boards.length === 0 && !query.isError ? <Text style={muted}>{t("boards.none")}</Text> : null}
      {boards.map((b) => (
        <SettingsRow
          key={b.device}
          label={b.device}
          hint={[
            b.online ? t("boards.online") : t("boards.offline"),
            b.fw ? t("boards.firmware", { version: b.fw }) : null,
            t("boards.lastSeen", { when: ago(t, b.lastSeen, now) }),
            b.prefs
              ? t("boards.prefs", {
                  sound: onOff(b.prefs.sound),
                  haptic: onOff(b.prefs.haptic),
                  brightness: b.prefs.brightness,
                }) + (b.prefs.dnd ? ` · ${t("boards.dnd")}` : "")
              : null,
          ]
            .filter(Boolean)
            .join(" · ")}
        />
      ))}
      {query.data?.usageAt ? <Text style={muted}>{t("boards.usage", { when: ago(t, query.data.usageAt, now) })}</Text> : null}
      {query.data && !query.data.canStop ? <Text style={muted}>{t("boards.noStop")}</Text> : null}
    </SettingsSection>
  );
}

/** The default workspace and agent profile for sessions started on a board. */
function Sessions({ theme, values, save }: HostProps & { values: Settings; save(next: Settings): void }) {
  const { t } = useTranslation(messages);
  const paseo = usePaseo();
  const workspaces = useQuery({
    queryKey: ["farol", "workspaces"],
    queryFn: () => paseo.workspaces.list(),
  });
  const profiles = useQuery({
    queryKey: ["farol", "profiles"],
    queryFn: async () => (await paseo.config.get()).config.agentProfiles ?? [],
  });
  const muted = { color: theme.colors.foregroundMuted, fontSize: 13 };
  const none = { label: t("session.none"), value: "" };
  const workspaceOptions = [
    none,
    ...(workspaces.data?.entries ?? []).map((w) => ({
      label: w.projectDisplayName && w.projectDisplayName !== w.name ? `${w.name} · ${w.projectDisplayName}` : w.name,
      value: w.id,
    })),
  ];
  // A saved choice that is no longer listed still shows (by id) until changed.
  if (values.workspaceId && !workspaceOptions.some((o) => o.value === values.workspaceId)) {
    workspaceOptions.push({ label: values.workspaceId, value: values.workspaceId });
  }
  const profileOptions = [
    none,
    ...(profiles.data ?? []).map((p) => ({ label: p.name || p.id, value: p.id })),
  ];
  if (values.profileId && !profileOptions.some((o) => o.value === values.profileId)) {
    profileOptions.push({ label: values.profileId, value: values.profileId });
  }
  return (
    <SettingsSection title={t("session.title")}>
      <Text style={muted}>{t("session.hint")}</Text>
      <SettingsSelect
        label={t("session.workspace")}
        value={values.workspaceId}
        options={workspaceOptions}
        disabled={workspaces.isLoading}
        onValueChange={(workspaceId) => save({ ...values, workspaceId })}
      />
      <SettingsSelect
        label={t("session.profile")}
        value={values.profileId}
        options={profileOptions}
        disabled={profiles.isLoading}
        onValueChange={(profileId) => save({ ...values, profileId })}
      />
      {profiles.data && profiles.data.length === 0 ? <Text style={muted}>{t("session.noProfiles")}</Text> : null}
    </SettingsSection>
  );
}

const BRIGHTNESS = ["0", "20", "40", "60", "80", "100"] as const;

function Preferences({ values, save, language }: { values: Settings; save(next: Settings): void; language: "zh" | "en" | null }) {
  const { t } = useTranslation(messages);
  const [timeZone, setTimeZone] = useState(values.timeZone);
  const [quietStart, setQuietStart] = useState(values.quietHours.start);
  const [quietEnd, setQuietEnd] = useState(values.quietHours.end);
  const valid = (s: string) => /^([01]\d|2[0-3]):[0-5]\d$/.test(s);
  const dirty =
    timeZone !== values.timeZone || quietStart !== values.quietHours.start || quietEnd !== values.quietHours.end;
  const set = (patch: Partial<Settings>) => save({ ...values, ...patch });
  const auto =
    values.boardLanguage === "auto" && language
      ? t("board.languageAutoNow", { language: t(`language.${language}`) })
      : t("board.languageAuto");
  return (
    <SettingsSection title={t("board.title")}>
      <SettingsSwitch
        label={t("board.actions")}
        hint={t("board.actionsHint")}
        value={values.allowActions}
        onValueChange={(v) => set({ allowActions: v })}
      />
      <SettingsSelect
        label={t("board.language")}
        value={values.boardLanguage}
        options={[
          { label: auto, value: "auto" as const },
          { label: t("language.zh"), value: "zh" as const },
          { label: t("language.en"), value: "en" as const },
        ]}
        onValueChange={(boardLanguage) => set({ boardLanguage })}
      />
      <SettingsSwitch
        label={t("board.preview")}
        hint={t("board.previewHint")}
        value={values.replyPreview}
        onValueChange={(v) => set({ replyPreview: v })}
      />
      <SettingsSwitch
        label={t("board.background")}
        value={values.includeBackground}
        onValueChange={(v) => set({ includeBackground: v })}
      />
      <SettingsSwitch label={t("board.sound")} value={values.sound} onValueChange={(v) => set({ sound: v })} />
      <SettingsSelect
        label={t("board.led")}
        value={String(values.ledBrightness) as (typeof BRIGHTNESS)[number]}
        options={BRIGHTNESS.map((v) => ({ label: `${v}%`, value: v }))}
        onValueChange={(v) => set({ ledBrightness: Number(v) })}
      />
      <SettingsSwitch
        label={t("board.quiet")}
        hint={t("board.quietHint")}
        value={values.quietHours.enabled}
        onValueChange={(v) => set({ quietHours: { ...values.quietHours, enabled: v } })}
      />
      <SettingsInput label={t("board.start")} initialValue={values.quietHours.start} placeholder="22:00" onChangeText={setQuietStart} />
      <SettingsInput label={t("board.end")} initialValue={values.quietHours.end} placeholder="08:00" onChangeText={setQuietEnd} />
      <SettingsInput
        label={t("board.timeZone")}
        hint={t("board.timeZoneHint")}
        initialValue={values.timeZone}
        placeholder="Asia/Shanghai"
        onChangeText={setTimeZone}
      />
      <SettingsAction
        label={t("board.saveLabel")}
        error={!valid(quietStart) || !valid(quietEnd) ? t("board.badTime") : null}
        actionLabel={t("board.save")}
        disabled={!dirty || !valid(quietStart) || !valid(quietEnd)}
        onPress={() =>
          set({
            timeZone: timeZone.trim(),
            quietHours: { ...values.quietHours, start: quietStart, end: quietEnd },
          })
        }
      />
    </SettingsSection>
  );
}

function useFarolSettings() {
  const state = useSettings(farolSettings);
  const values = state.status === "ready" ? state.values : null;
  const save = (next: Settings) => {
    if (state.status === "ready") void state.save(next, state.revision);
  };
  return { state, values, save };
}

function Shell({ theme, layout, children }: HostProps & { children: ReactNode }) {
  return (
    <ScrollView
      style={{ flex: 1, backgroundColor: theme.colors.surface0 }}
      contentContainerStyle={{ padding: layout.compact ? 16 : 24, gap: 24 }}
    >
      {children}
    </ScrollView>
  );
}

function Status({ theme, state }: HostProps & { state: ReturnType<typeof useFarolSettings>["state"] }) {
  const { t } = useTranslation(messages);
  const style = { color: theme.colors.foregroundMuted, fontSize: 13 };
  if (state.status === "loading") return <Text style={style}>{t("status.loading")}</Text>;
  if (state.status === "error" || state.status === "invalid") {
    return <Text style={{ ...style, color: theme.colors.statusDanger }}>{t("status.failed", { error: state.error })}</Text>;
  }
  if (state.saveError) return <Text style={{ ...style, color: theme.colors.statusDanger }}>{state.saveError}</Text>;
  return null;
}

export function SettingsScreen({ theme, layout }: PluginSurfaceProps) {
  const { state, values, save } = useFarolSettings();
  const status = useRpc(statusRpc);
  const language = useQuery({
    queryKey: ["farol", "status"],
    queryFn: () => status({}),
    refetchInterval: 5000,
  }).data?.boardLanguage;
  const key = state.status === "ready" ? state.revision : "-";
  return (
    <Shell theme={theme} layout={layout}>
      <Status theme={theme} layout={layout} state={state} />
      <Boards theme={theme} layout={layout} />
      {values ? <Sessions key={`s${key}`} theme={theme} layout={layout} values={values} save={save} /> : null}
      {values ? <Preferences key={key} values={values} save={save} language={language ?? null} /> : null}
    </Shell>
  );
}
