import { defineRpc, defineSettings } from "@getpaseo/plugin";
import { z } from "zod";
import { BoardPrefsSchema } from "./protocol";

const clock = z.string().regex(/^([01]\d|2[0-3]):[0-5]\d$/, "HH:MM");

export const SettingsSchema = z.object({
  /** The address boards connect to: the daemon's WebSocket as seen from the board. */
  address: z.string().default(""),
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
    /** Sound as last switched on a board, when that is newer than this page's setting. */
    boardSound: z.boolean().nullable(),
  }),
});

/**
 * Builds the connection code the board scans. The password passes through and
 * is not kept: the daemon only stores its hash.
 */
export const connectCodeRpc = defineRpc({
  name: "farol.connect_code",
  input: z.object({
    address: z.string().min(1),
    password: z.string(),
    name: z.string().optional(),
  }),
  output: z.discriminatedUnion("ok", [
    z.object({
      ok: z.literal(true),
      url: z.string(),
      payload: z.string(),
      /** One string per module row, "1" dark and "0" light. */
      rows: z.array(z.string()),
    }),
    z.object({ ok: z.literal(false), error: z.string() }),
  ]),
});

export const TIMELINE_KIND = "farol-action";
export const TimelineRowSchema = z.object({
  action: z.enum(["answer", "stop"]),
  text: z.string(),
  device: z.string(),
});
export type TimelineRow = z.infer<typeof TimelineRowSchema>;
