// The plugin's interface text, English and Simplified Chinese. Paseo picks the
// app's language; anything missing falls back to English.
import { defineTranslations } from "@getpaseo/plugin/client";

const en = {
  ago: {
    now: "just now",
    minutes: "{{n}} min ago",
    hours: "{{n}} h ago",
  },
  boards: {
    title: "Recent boards",
    none: "No board has connected yet.",
    unreadable: "Can't read the plugin's state: {{error}}",
    online: "online",
    offline: "offline",
    firmware: "firmware {{version}}",
    lastSeen: "last seen {{when}}",
    prefs: "sound {{sound}} · haptics {{haptic}} · brightness {{brightness}}%",
    on: "on",
    off: "off",
    dnd: "do not disturb",
    usage: "Usage data from {{when}}",
    noStop: "The paseo CLI isn't on the daemon's PATH, so boards can't stop turns.",
  },
  session: {
    title: "New sessions from a board",
    hint: "Hold the AI key on a board and speak (Paseo's dictation), or type. The session starts here.",
    workspace: "Workspace",
    profile: "Agent profile",
    none: "Off",
    noProfiles: "Add an agent profile in Paseo first (Settings › Agent profiles).",
    loading: "Loading…",
  },
  board: {
    title: "Boards",
    actions: "Let boards act on agents",
    actionsHint: "Answer questions, permissions and plans, stop a turn, start a session. Off makes boards read-only.",
    preview: "Reply preview",
    previewHint: "Finished cards show the start of the last reply.",
    background: "Include background workspaces",
    sound: "Sound",
    led: "LED brightness",
    quiet: "Quiet hours",
    quietHint: "No sound or vibration in this time.",
    start: "Start",
    end: "End",
    timeZone: "Time zone",
    timeZoneHint: "IANA name, e.g. Asia/Shanghai. Empty follows this Mac.",
    save: "Save",
    saveLabel: "Quiet hours and time zone",
    badTime: "Use HH:MM",
    language: "Board language",
    languageAuto: "Follow Paseo",
    languageAutoNow: "Follow Paseo ({{language}})",
  },
  language: {
    zh: "中文",
    en: "English",
  },
  status: {
    loading: "Loading settings…",
    failed: "Can't read the settings: {{error}}",
  },
  timeline: {
    answer: "Answered on {{where}}: {{text}}",
    stop: "Stopped this turn on {{where}}",
  },
};

const zh: typeof en = {
  ago: {
    now: "刚刚",
    minutes: "{{n}} 分钟前",
    hours: "{{n}} 小时前",
  },
  boards: {
    title: "最近的板子",
    none: "还没有板子连过来。",
    unreadable: "读不到插件状态：{{error}}",
    online: "在线",
    offline: "离线",
    firmware: "固件 {{version}}",
    lastSeen: "最近一次 {{when}}",
    prefs: "声音{{sound}} · 振动{{haptic}} · 亮度 {{brightness}}%",
    on: "开",
    off: "关",
    dnd: "勿扰",
    usage: "额度数据更新于 {{when}}",
    noStop: "daemon 的 PATH 里找不到 paseo 命令行，板子不能停止一轮。",
  },
  session: {
    title: "从板子开始新会话",
    hint: "在板子上按住 AI 键说话（用 Paseo 的语音输入），或者打字。会话开在这里选的地方。",
    workspace: "Workspace",
    profile: "Agent 配置",
    none: "关闭",
    noProfiles: "先在 Paseo 里添加一个 agent 配置（设置 › Agent 配置）。",
    loading: "正在读取…",
  },
  board: {
    title: "板子",
    actions: "允许板子操作 agent",
    actionsHint: "回答问题、权限和计划，停止一轮，开始新会话。关掉后板子只读。",
    preview: "显示回复预览",
    previewHint: "完成卡上显示最后一条回复的开头。",
    background: "包含后台 workspace",
    sound: "声音",
    led: "LED 亮度",
    quiet: "安静时段",
    quietHint: "这段时间里不响、不振。",
    start: "开始",
    end: "结束",
    timeZone: "时区",
    timeZoneHint: "IANA 名称，例如 Asia/Shanghai；留空跟随这台 Mac。",
    save: "保存",
    saveLabel: "安静时段和时区",
    badTime: "时间格式应为 HH:MM",
    language: "板子的语言",
    languageAuto: "跟随 Paseo",
    languageAutoNow: "跟随 Paseo（{{language}}）",
  },
  language: {
    zh: "中文",
    en: "English",
  },
  status: {
    loading: "正在读取设置…",
    failed: "设置读取失败：{{error}}",
  },
  timeline: {
    answer: "在 {{where}} 上回答：{{text}}",
    stop: "在 {{where}} 上停止了这一轮",
  },
};

export type Messages = typeof en;

export const messages = defineTranslations({ resources: { en, "zh-CN": zh } });
