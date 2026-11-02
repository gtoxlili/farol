import { defineRpc, defineSettings } from "@getpaseo/plugin";
import { z } from "zod";
import { BoardPrefsSchema } from "./protocol";

const clock = z.string().regex(/^([01]\d|2[0-3]):[0-5]\d$/, "HH:MM");

export const SettingsSchema = z.object({
  /** Let boards answer questions and stop turns. Off makes every board read-only. */
  allowActions: z.boolean().default(true),
  /** Show the start of the last reply on finished cards. */
  replyPreview: z.boolean().default(true),
  /** Include agents in background workspaces. */
  includeBackground: z.boolean().default(false),
  sound: z.boolean().default(true),
  /** Interaction module LED strip, percent. */
  ledBrightness: z.number().int().min(0).max(100).default(60),
  quietHours: z
    .object({ enabled: z.boolean(), start: clock, end: clock })
    .default({ enabled: false, start: "22:00", end: "08:00" }),
  /** IANA zone for the board's clock; empty follows the Mac. */
  timeZone: z.string().default(""),
  /** Where sessions started on a board go (a workspace id); empty turns that off. */
  workspaceId: z.string().default(""),
  /** The agent profile those sessions use (an id from Paseo's agent profiles). */
  profileId: z.string().default(""),
  /** The boards' language: "auto" follows the Paseo app. */
  boardLanguage: z.enum(["auto", "zh", "en"]).default("auto"),
});
export type Settings = z.infer<typeof SettingsSchema>;

export const DEFAULT_SETTINGS: Settings = SettingsSchema.parse({});

export const farolSettings = defineSettings({
  id: "farol",
  scope: "host",
  version: 1,
  schema: SettingsSchema,
});

export const BoardSchema = z.object({
  device: z.string(),
  firstSeen: z.number(),
  lastSeen: z.number(),
  online: z.boolean(),
  focus: z.string().nullable(),
  /** Firmware version and the screen in front, as the board last reported. */
  fw: z.string().nullable(),
  screen: z.string().nullable(),
  /** What the board's own control centre last reported. */
  prefs: BoardPrefsSchema.nullable(),
});
export type Board = z.infer<typeof BoardSchema>;

/** Settings page: recent boards and what this host can do. */
export const statusRpc = defineRpc({
  name: "farol.status",
  input: z.object({}),
  output: z.object({
    host: z.string(),
    boards: z.array(BoardSchema),
    canStop: z.boolean(),
    /** When provider usage was last fetched, epoch ms. */
    usageAt: z.number().nullable(),
    /** The language boards use now ("zh", "en"), or null while unknown. */
    boardLanguage: z.enum(["zh", "en"]).nullable(),
  }),
});

/** The Paseo app reports its language (boards follow it unless set otherwise). */
export const languageRpc = defineRpc({
  name: "farol.language",
  input: z.object({ locale: z.string().max(40) }),
  output: z.object({ ok: z.literal(true) }),
});

export const TIMELINE_KIND = "farol-action";
export const TimelineRowSchema = z.object({
  action: z.enum(["answer", "stop"]),
  text: z.string(),
  device: z.string(),
});
export type TimelineRow = z.infer<typeof TimelineRowSchema>;
