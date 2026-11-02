// Settings › Plugins › farol › Settings. Laid out the way Paseo's own settings
// are: a muted heading per section, rows on a card, status as a small pill.
// Paseo owns the header, scrolling, safe areas and the centred column, so this
// renders sections only. Boards connect to Paseo on their own (direct or
// relay), so nothing here pairs them.
import type { PluginSurfaceProps } from "@getpaseo/plugin/client";
import { usePaseo, useRpc, useSettings, useTranslation } from "@getpaseo/plugin/client";
import {
  Loading,
  SettingsAction,
  SettingsCard,
  SettingsInput,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
  SettingsSwitch,
} from "@getpaseo/plugin/client/ui";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Text, View } from "react-native";
import { farolSettings, type Settings, statusRpc } from "../shared/settings";
import { type Messages, messages } from "./messages";
import { WorkspacePicker } from "./workspace-picker";

type Theme = PluginSurfaceProps["theme"];
type T = ReturnType<typeof useTranslation<Messages>>["t"];

function ago(t: T, ms: number, now: number): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return t("ago.now");
  if (s < 3600) return t("ago.minutes", { n: Math.round(s / 60) });
  if (s < 86400) return t("ago.hours", { n: Math.round(s / 3600) });
  return new Date(ms).toLocaleDateString();
}

/** `color` at `alpha`, for a pill's fill; theme colors are #rrggbb. */
function tint(color: string, alpha: number): string {
  if (!/^#[0-9a-f]{6}$/i.test(color)) return "transparent";
  return `${color}${Math.round(alpha * 255)
    .toString(16)
    .padStart(2, "0")}`;
}

/** A status pill, as Paseo shows "running" on a plugin. */
function Pill({ theme, text, on }: { theme: Theme; text: string; on: boolean }) {
  const color = on ? theme.colors.statusSuccess : theme.colors.foregroundMuted;
  return (
    <View
      style={{
        backgroundColor: on ? tint(color, 0.16) : theme.colors.surface2,
        borderRadius: 999,
        paddingHorizontal: 10,
        paddingVertical: 3,
      }}
    >
      <Text style={{ color, fontSize: 13 }}>{text}</Text>
    </View>
  );
}

function useStatus() {
  const status = useRpc(statusRpc);
  return useQuery({
    queryKey: ["farol", "status"],
    queryFn: () => status({}),
    refetchInterval: 5000,
  });
}

function Boards({ theme }: { theme: Theme }) {
  const { t } = useTranslation(messages);
  const query = useStatus();
  const now = Date.now();
  const boards = query.data?.boards ?? [];
  const onOff = (on: boolean) => (on ? t("boards.on") : t("boards.off"));
  return (
    <SettingsSection title={t("boards.title")}>
      <SettingsCard>
        {query.isError ? (
          <SettingsRow label={t("boards.title")} error={t("boards.unreadable", { error: String(query.error) })} />
        ) : null}
        {boards.length === 0 && !query.isError ? <SettingsRow label={t("boards.none")} hint={t("boards.noneHint")} /> : null}
        {boards.map((b) => (
          <SettingsRow
            key={b.device}
            label={b.device.startsWith("farol-") ? b.device : `farol-${b.device}`}
            hint={[
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
          >
            <Pill theme={theme} on={b.online} text={b.online ? t("boards.online") : t("boards.offline")} />
          </SettingsRow>
        ))}
        {query.data && !query.data.canStop ? <SettingsRow label={t("boards.stop")} hint={t("boards.noStop")} /> : null}
      </SettingsCard>
    </SettingsSection>
  );
}

/** The default workspace and agent profile for sessions started on a board. */
function Sessions({ theme, values, save }: { theme: Theme; values: Settings; save(next: Settings): void }) {
  const { t } = useTranslation(messages);
  const paseo = usePaseo();
  const profiles = useQuery({
    queryKey: ["farol", "profiles"],
    queryFn: async () => (await paseo.config.get()).config.agentProfiles ?? [],
  });
  const profileOptions = [
    { label: t("session.none"), value: "" },
    ...(profiles.data ?? []).map((p) => ({ label: p.name || p.id, value: p.id })),
  ];
  if (values.profileId && !profileOptions.some((o) => o.value === values.profileId)) {
    profileOptions.push({ label: values.profileId, value: values.profileId });
  }
  return (
    <SettingsSection title={t("session.title")} info={t("session.hint")}>
      <SettingsCard>
        <WorkspacePicker theme={theme} value={values.workspaceId} onChange={(workspaceId) => save({ ...values, workspaceId })} />
        {profiles.data && profiles.data.length === 0 ? (
          <SettingsRow label={t("session.profile")} hint={t("session.noProfiles")} />
        ) : (
          <SettingsSelect
            label={t("session.profile")}
            value={values.profileId}
            options={profileOptions}
            disabled={profiles.isLoading}
            onValueChange={(profileId) => save({ ...values, profileId })}
          />
        )}
      </SettingsCard>
    </SettingsSection>
  );
}

const BRIGHTNESS = ["0", "20", "40", "60", "80", "100"] as const;

function Behaviour({
  values,
  save,
  language,
}: {
  values: Settings;
  save(next: Settings): void;
  language: "zh" | "en" | null;
}) {
  const { t } = useTranslation(messages);
  const set = (patch: Partial<Settings>) => save({ ...values, ...patch });
  const auto =
    values.boardLanguage === "auto" && language
      ? t("board.languageAutoNow", { language: t(`language.${language}`) })
      : t("board.languageAuto");
  return (
    <SettingsSection title={t("board.title")}>
      <SettingsCard>
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
      </SettingsCard>
    </SettingsSection>
  );
}

function QuietHours({ values, save }: { values: Settings; save(next: Settings): void }) {
  const { t } = useTranslation(messages);
  const [timeZone, setTimeZone] = useState(values.timeZone);
  const [quietStart, setQuietStart] = useState(values.quietHours.start);
  const [quietEnd, setQuietEnd] = useState(values.quietHours.end);
  const valid = (s: string) => /^([01]\d|2[0-3]):[0-5]\d$/.test(s);
  const dirty =
    timeZone !== values.timeZone || quietStart !== values.quietHours.start || quietEnd !== values.quietHours.end;
  return (
    <SettingsSection title={t("quiet.title")}>
      <SettingsCard>
        <SettingsSwitch
          label={t("board.quiet")}
          hint={t("board.quietHint")}
          value={values.quietHours.enabled}
          onValueChange={(v) => save({ ...values, quietHours: { ...values.quietHours, enabled: v } })}
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
            save({
              ...values,
              timeZone: timeZone.trim(),
              quietHours: { ...values.quietHours, start: quietStart, end: quietEnd },
            })
          }
        />
      </SettingsCard>
    </SettingsSection>
  );
}

export function SettingsScreen({ theme }: PluginSurfaceProps) {
  const { t } = useTranslation(messages);
  const state = useSettings(farolSettings);
  const language = useStatus().data?.boardLanguage ?? null;
  if (state.status === "loading") return <Loading />;
  if (state.status === "error" || state.status === "invalid") {
    return (
      <SettingsSection title={t("status.title")}>
        <SettingsCard>
          <SettingsRow label={t("status.title")} error={t("status.failed", { error: state.error })} />
        </SettingsCard>
      </SettingsSection>
    );
  }
  const values = state.values;
  const save = (next: Settings) => void state.save(next, state.revision);
  return (
    <>
      {state.saveError ? (
        <SettingsSection title={t("status.title")}>
          <SettingsCard>
            <SettingsRow label={t("status.title")} error={state.saveError} />
          </SettingsCard>
        </SettingsSection>
      ) : null}
      <Boards theme={theme} />
      <Sessions key={`s${state.revision}`} theme={theme} values={values} save={save} />
      <Behaviour values={values} save={save} language={language} />
      <QuietHours key={`q${state.revision}`} values={values} save={save} />
    </>
  );
}
