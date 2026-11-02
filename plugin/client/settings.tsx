// Settings › Plugins › Farol, and the "add a board" screen it shares with the
// Command Center item. React Native primitives and host settings rows only.
import type { PluginScreenProps, PluginSurfaceProps } from "@getpaseo/plugin/client";
import { useRpc, useSettings } from "@getpaseo/plugin/client";
import {
  ScrollView,
  SettingsAction,
  SettingsInput,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
  SettingsSwitch,
} from "@getpaseo/plugin/client/ui";
import { useMutation, useQuery } from "@tanstack/react-query";
import { type ReactNode, useEffect, useMemo, useState } from "react";
import { Text, View } from "react-native";
import { connectCodeRpc, farolSettings, type Settings, statusRpc } from "../shared/settings";
import { QrView } from "./qr-view";

type HostProps = Pick<PluginSurfaceProps, "theme" | "layout">;

function ago(ms: number, now: number): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return "刚刚";
  if (s < 3600) return `${Math.round(s / 60)} 分钟前`;
  if (s < 86400) return `${Math.round(s / 3600)} 小时前`;
  return new Date(ms).toLocaleDateString();
}

/** Address, password once, and the connection code. The password is never saved. */
export function AddDevice({ theme, layout, address }: HostProps & { address: string }) {
  const makeCode = useRpc(connectCodeRpc);
  const [password, setPassword] = useState("");
  const code = useMutation({ mutationFn: makeCode });
  // Forget the password and the code when the screen goes away.
  useEffect(() => () => setPassword(""), []);
  const text = useMemo(
    () => ({
      body: { color: theme.colors.foreground, fontSize: 14 },
      muted: { color: theme.colors.foregroundMuted, fontSize: 13 },
      danger: { color: theme.colors.statusDanger, fontSize: 13 },
    }),
    [theme],
  );
  const result = code.data;
  const qrSize = layout.compact ? 260 : 320;
  return (
    <SettingsSection title="添加设备">
      <SettingsRow
        label="板子要连的地址"
        hint={address || "先在下方“连接”里填写板子能访问的地址"}
      />
      <SettingsInput
        label="Daemon 密码"
        hint="只用来生成连接码，插件不保存。板子会把它存在自己的闪存里。"
        secureTextEntry
        initialValue=""
        placeholder="daemon password"
        onChangeText={(value) => {
          setPassword(value);
          code.reset();
        }}
      />
      <SettingsAction
        label="连接码"
        hint="在板子上扫码，或在板子上手动输入地址和密码"
        actionLabel={result?.ok ? "重新生成" : "生成"}
        disabled={!address || code.isPending}
        onPress={() => code.mutate({ address, password })}
      />
      {code.error ? <Text style={text.danger}>{String(code.error)}</Text> : null}
      {result && !result.ok ? <Text style={text.danger}>{result.error}</Text> : null}
      {result?.ok ? (
        <View style={{ alignItems: "center", gap: 12, paddingVertical: 16 }}>
          <QrView rows={result.rows} size={qrSize} />
          <Text style={text.body} selectable>
            {result.url}
          </Text>
          <Text style={text.muted}>
            连接码里有 daemon 密码，等于这台 Paseo 的全部权限。扫完就关掉这一页；板子丢了，就改 daemon 密码。
          </Text>
        </View>
      ) : null}
    </SettingsSection>
  );
}

function Boards({ theme, onBoardSound }: HostProps & { onBoardSound?(sound: boolean): void }) {
  const status = useRpc(statusRpc);
  const query = useQuery({
    queryKey: ["farol", "status"],
    queryFn: () => status({}),
    refetchInterval: 5000,
  });
  const muted = { color: theme.colors.foregroundMuted, fontSize: 13 };
  const now = Date.now();
  const boards = query.data?.boards ?? [];
  // A board switched sound in its control centre: save it here so Paseo's setting follows.
  const boardSound = query.data?.boardSound ?? null;
  useEffect(() => {
    if (boardSound !== null) onBoardSound?.(boardSound);
  }, [boardSound]);
  return (
    <SettingsSection title="最近的板子">
      {query.isError ? <Text style={muted}>读不到插件状态：{String(query.error)}</Text> : null}
      {boards.length === 0 && !query.isError ? (
        <Text style={muted}>还没有板子连过来。</Text>
      ) : null}
      {boards.map((b) => (
        <SettingsRow
          key={b.device}
          label={b.device}
          hint={[
            b.online ? "在线" : "离线",
            `最近一次 ${ago(b.lastSeen, now)}`,
            b.prefs
              ? `声音${b.prefs.sound ? "开" : "关"} · 振动${b.prefs.haptic ? "开" : "关"} · 亮度 ${b.prefs.brightness}%${b.prefs.dnd ? " · 勿扰" : ""}`
              : null,
          ]
            .filter(Boolean)
            .join(" · ")}
        />
      ))}
      {query.data && !query.data.canStop ? (
        <Text style={muted}>找不到 paseo 命令行，板子上的“停止”不可用。</Text>
      ) : null}
    </SettingsSection>
  );
}

const BRIGHTNESS = ["0", "20", "40", "60", "80", "100"] as const;

function Preferences({ values, save }: { values: Settings; save(next: Settings): void }) {
  const [address, setAddress] = useState(values.address);
  const [timeZone, setTimeZone] = useState(values.timeZone);
  const [quietStart, setQuietStart] = useState(values.quietHours.start);
  const [quietEnd, setQuietEnd] = useState(values.quietHours.end);
  const valid = (t: string) => /^([01]\d|2[0-3]):[0-5]\d$/.test(t);
  const dirty =
    address !== values.address ||
    timeZone !== values.timeZone ||
    quietStart !== values.quietHours.start ||
    quietEnd !== values.quietHours.end;
  const set = (patch: Partial<Settings>) => save({ ...values, ...patch });
  return (
    <>
      <SettingsSection title="连接">
        <SettingsInput
          label="地址"
          hint="板子能访问到的 daemon 地址，例如 wss://paseo.example.net/ws。daemon.hostnames 要包含这个域名。"
          initialValue={values.address}
          placeholder="wss://paseo.example.net/ws"
          onChangeText={setAddress}
        />
      </SettingsSection>
      <SettingsSection title="板子">
        <SettingsSwitch
          label="允许板子操作 agent"
          hint="回答问题、权限和计划，停止一轮。关掉后板子只读。"
          value={values.allowActions}
          onValueChange={(v) => set({ allowActions: v })}
        />
        <SettingsSwitch
          label="显示回复预览"
          hint="完成卡上显示最后一条回复的开头"
          value={values.replyPreview}
          onValueChange={(v) => set({ replyPreview: v })}
        />
        <SettingsSwitch
          label="包含后台工作区"
          value={values.includeBackground}
          onValueChange={(v) => set({ includeBackground: v })}
        />
        <SettingsSwitch label="声音" value={values.sound} onValueChange={(v) => set({ sound: v })} />
        <SettingsSelect
          label="LED 亮度"
          value={String(values.ledBrightness) as (typeof BRIGHTNESS)[number]}
          options={BRIGHTNESS.map((v) => ({ label: `${v}%`, value: v }))}
          onValueChange={(v) => set({ ledBrightness: Number(v) })}
        />
        <SettingsSwitch
          label="安静时段"
          hint="这段时间里不响、不振"
          value={values.quietHours.enabled}
          onValueChange={(v) => set({ quietHours: { ...values.quietHours, enabled: v } })}
        />
        <SettingsInput label="开始" initialValue={values.quietHours.start} placeholder="22:00" onChangeText={setQuietStart} />
        <SettingsInput label="结束" initialValue={values.quietHours.end} placeholder="08:00" onChangeText={setQuietEnd} />
        <SettingsInput
          label="时区"
          hint="IANA 名称，例如 Asia/Shanghai；留空跟随这台 Mac"
          initialValue={values.timeZone}
          placeholder="Asia/Shanghai"
          onChangeText={setTimeZone}
        />
        <SettingsAction
          label="地址、安静时段和时区"
          error={!valid(quietStart) || !valid(quietEnd) ? "时间格式应为 HH:MM" : null}
          actionLabel="保存"
          disabled={!dirty || !valid(quietStart) || !valid(quietEnd)}
          onPress={() =>
            set({
              address: address.trim(),
              timeZone: timeZone.trim(),
              quietHours: { ...values.quietHours, start: quietStart, end: quietEnd },
            })
          }
        />
      </SettingsSection>
    </>
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
  const style = { color: theme.colors.foregroundMuted, fontSize: 13 };
  if (state.status === "loading") return <Text style={style}>正在读取设置…</Text>;
  if (state.status === "error" || state.status === "invalid") {
    return <Text style={{ ...style, color: theme.colors.statusDanger }}>设置读取失败：{state.error}</Text>;
  }
  if (state.saveError) return <Text style={{ ...style, color: theme.colors.statusDanger }}>{state.saveError}</Text>;
  return null;
}

export function SettingsScreen({ theme, layout }: PluginSurfaceProps) {
  const { state, values, save } = useFarolSettings();
  return (
    <Shell theme={theme} layout={layout}>
      <Status theme={theme} layout={layout} state={state} />
      {values ? <AddDevice theme={theme} layout={layout} address={values.address} /> : null}
      <Boards
        theme={theme}
        layout={layout}
        onBoardSound={(sound) => {
          if (values && values.sound !== sound) save({ ...values, sound });
        }}
      />
      {values ? <Preferences key={state.status === "ready" ? state.revision : "-"} values={values} save={save} /> : null}
    </Shell>
  );
}

export function AddDeviceScreen({ theme, layout }: PluginScreenProps) {
  const { state, values } = useFarolSettings();
  return (
    <Shell theme={theme} layout={layout}>
      <Status theme={theme} layout={layout} state={state} />
      {values ? <AddDevice theme={theme} layout={layout} address={values.address} /> : null}
      {values && !values.address ? (
        <Text style={{ color: theme.colors.foregroundMuted, fontSize: 13 }}>
          地址在 设置 › 插件 › Farol 里填写。
        </Text>
      ) : null}
    </Shell>
  );
}
