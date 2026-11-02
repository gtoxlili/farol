// Board settings both ways: Paseo's settings ride every snapshot with a rev;
// a board's control centre reports prefs, and its sound switch becomes global.
import { describe, expect, it } from "vitest";
import { FocusSchema, NextSchema } from "../shared/protocol";
import { DEFAULT_SETTINGS } from "../shared/settings";
import { boardSettingsOf } from "../server/digest";
import { Farol } from "../server/farol";

const quiet = { log: { info() {}, warn() {} } };
const prefs = { sound: false, haptic: true, brightness: 70, dnd: false };

async function settingsOf(farol: Farol, device = "b") {
  const s = await farol.stream.next(device, 0, null);
  expect(s.v).toBe(1);
  return s.settings!;
}

describe("board settings", () => {
  it("accepts prefs as paseo.rs sends them on farol.next and farol.focus", () => {
    expect(NextSchema.parse({ v: 1, cursor: 3, device: "3a1f", prefs }).prefs).toEqual(prefs);
    expect(FocusSchema.parse({ device: "3a1f", prefs }).prefs).toEqual(prefs);
  });

  it("quiet hours become minutes after midnight; off or empty is 0..0", () => {
    const on = { ...DEFAULT_SETTINGS, quietHours: { enabled: true, start: "22:30", end: "07:15" } };
    expect(boardSettingsOf(on, 1)).toEqual({ rev: 1, sound: true, ledBrightness: 60, quietStart: 1350, quietEnd: 435 });
    expect(boardSettingsOf(DEFAULT_SETTINGS, 1)).toMatchObject({ quietStart: 0, quietEnd: 0 });
  });

  it("a settings change bumps the rev; an unrelated one does not", async () => {
    const farol = new Farol(quiet.log);
    farol.setSettings(DEFAULT_SETTINGS);
    const first = await settingsOf(farol);
    farol.setSettings({ ...DEFAULT_SETTINGS, replyPreview: false });
    expect((await settingsOf(farol, "c")).rev).toBe(first.rev);
    farol.setSettings({ ...DEFAULT_SETTINGS, replyPreview: false, ledBrightness: 20 });
    const second = await settingsOf(farol, "d");
    expect(second.rev).toBeGreaterThan(first.rev);
    expect(second.ledBrightness).toBe(20);
    await farol.close();
  });

  it("a board's own settings show with the board and leave Paseo's settings alone", async () => {
    const farol = new Farol(quiet.log);
    farol.setSettings(DEFAULT_SETTINGS);
    const before = await settingsOf(farol);
    farol.notePrefs("b", prefs);
    expect(farol.stream.boards().find((b) => b.device === "b")?.prefs).toEqual(prefs);
    const after = await settingsOf(farol, "c");
    expect(after).toEqual(before);
    // An edit in Paseo reaches every board through a new rev.
    farol.setSettings({ ...DEFAULT_SETTINGS, sound: false });
    const edited = await settingsOf(farol, "d");
    expect(edited.sound).toBe(false);
    expect(edited.rev).toBeGreaterThan(before.rev);
    await farol.close();
  });
});
