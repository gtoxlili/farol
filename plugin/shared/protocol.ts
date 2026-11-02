// farol/1: the JSON the Farol plugin exchanges with the board through plugin
// RPCs. The board's Rust model (rust/logic/src/model.rs) is the reader of
// record; these schemas mirror its serde field names. See protocol/README.md.
import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";

export const PROTOCOL_VERSION = 1;

/** Deck order, most urgent first. */
export const STATES = ["call", "fail", "done", "work", "seen"] as const;
export const StateSchema = z.enum(STATES);
export type State = z.infer<typeof StateSchema>;

const u8 = z.number().int().min(0).max(255);
const u16 = z.number().int().min(0).max(65535);

export const AskSchema = z.object({
  kind: z.enum(["question", "permission", "plan"]),
  requestId: z.string(),
  text: z.string(),
  options: z.array(z.string()),
});
export type Ask = z.infer<typeof AskSchema>;

export const TodoSchema = z.object({
  done: u16,
  total: u16,
  current: z.string(),
});
export type Todo = z.infer<typeof TodoSchema>;

export const AgentSchema = z.object({
  id: z.string(),
  title: z.string().optional(),
  project: z.string().optional(),
  model: z.string().optional(),
  state: StateSchema,
  since: z.number().int().nullable().optional(),
  now: z.string().optional(),
  recent: z.array(z.string()).optional(),
  todo: TodoSchema.optional(),
  ctx: u8.optional(),
  cost: z.string().optional(),
  ask: AskSchema.optional(),
  body: z.string().optional(),
  error: z.string().optional(),
  stoppable: z.boolean().optional(),
});
export type Agent = z.infer<typeof AgentSchema>;

export const WindowSchema = z.object({
  label: z.string(),
  used: u8,
  resets: z.number().int().nullable().optional(),
});
export type Window = z.infer<typeof WindowSchema>;

export const FuelSchema = z.object({
  label: z.string(),
  left: u8,
});
export type Fuel = z.infer<typeof FuelSchema>;

/** Board settings made in Paseo (sound, LED, quiet hours); boards apply a newer `rev`. */
export const BoardSettingsSchema = z.object({
  rev: z.number().int().nonnegative(),
  sound: z.boolean(),
  ledBrightness: z.number().int().min(0).max(100),
  /** Minutes after local midnight; equal start and end mean no quiet hours. */
  quietStart: z.number().int().min(0).max(1439),
  quietEnd: z.number().int().min(0).max(1439),
});
export type BoardSettings = z.infer<typeof BoardSettingsSchema>;

/** Settings changed on the board's control centre, resent until a `farol.next` answer. */
export const BoardPrefsSchema = z.object({
  sound: z.boolean(),
  haptic: z.boolean(),
  /** Screen brightness, 5..100. */
  brightness: z.number().int().min(0).max(255),
  dnd: z.boolean(),
});
export type BoardPrefs = z.infer<typeof BoardPrefsSchema>;

export const SnapshotSchema = z.object({
  v: z.literal(PROTOCOL_VERSION),
  rev: z.number().int().nonnegative(),
  now: z.number().int(),
  tz: z.number().int(),
  host: z.string(),
  readOnly: z.boolean(),
  idle: z.boolean().optional(),
  settings: BoardSettingsSchema.optional(),
  agents: z.array(AgentSchema),
  usage: z.array(WindowSchema),
  fuel: FuelSchema.optional(),
});
export type Snapshot = z.infer<typeof SnapshotSchema>;

const deviceId = z.string().max(64);
/** The board's screen in front: "usage", "home" or "deck" (lenient for newer boards). */
const screen = z.string().max(16);

export const NextSchema = z.object({
  v: z.number().int(),
  cursor: z.number().int().nonnegative(),
  focus: z.string().nullable().optional(),
  device: deviceId,
  /** Absent from boards older than firmware 1.0.0. */
  screen: screen.optional(),
  fw: z.string().max(32).optional(),
  prefs: BoardPrefsSchema.optional(),
});
export type Next = z.infer<typeof NextSchema>;

export const FocusSchema = z.object({
  focus: z.string().nullable().optional(),
  device: deviceId,
  screen: screen.optional(),
  prefs: BoardPrefsSchema.optional(),
});
export type Focus = z.infer<typeof FocusSchema>;

export const ActSchema = z.object({
  agent: z.string(),
  /** The acting board as Paseo knows it ("farol-3a1f"); absent from older boards. */
  device: z.string().max(80).optional(),
  kind: z.enum(["answer", "stop", "seen"]),
  requestId: z.string().optional(),
  choice: u8.optional(),
});
export type Act = z.infer<typeof ActSchema>;

export const ACT_REASONS = ["already_resolved", "read_only", "not_found", "failed"] as const;
export type ActReason = (typeof ACT_REASONS)[number];

export const ActResultSchema = z.object({
  ok: z.boolean(),
  reason: z.enum(ACT_REASONS).optional(),
  message: z.string().optional(),
});
export type ActResult = z.infer<typeof ActResultSchema>;

/** Long poll: answers when the board's view changed past `cursor`, else idle within 20 s. */
export const nextRpc = defineRpc({ name: "farol.next", input: NextSchema, output: SnapshotSchema });

/** The card on screen changed; wakes the device's waiting `farol.next`. */
export const focusRpc = defineRpc({
  name: "farol.focus",
  input: FocusSchema,
  output: z.object({ ok: z.boolean() }),
});

/** Answer, stop or mark seen. */
export const actRpc = defineRpc({ name: "farol.act", input: ActSchema, output: ActResultSchema });
