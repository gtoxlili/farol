// A board turning to its usage screen refreshes usage at once, at most once a minute.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PaseoApi } from "@getpaseo/client";
import { Farol } from "../server/farol";
import { Observer, USAGE_MIN_GAP_MS } from "../server/observer";

function fakePaseo() {
  const listUsage = vi.fn(async () => ({ requestId: "r", fetchedAt: "", providers: [] }));
  return { paseo: { providers: { listUsage } } as unknown as PaseoApi, listUsage };
}

describe("usage refresh", () => {
  beforeEach(() => vi.useFakeTimers({ now: 10_000_000 }));
  afterEach(() => vi.useRealTimers());

  it("refreshes now, then not again within a minute", async () => {
    const { paseo, listUsage } = fakePaseo();
    const observer = new Observer(paseo, () => {}, { info() {}, warn() {} });
    observer.refreshUsageSoon();
    observer.refreshUsageSoon();
    expect(listUsage).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(USAGE_MIN_GAP_MS + 1);
    observer.refreshUsageSoon();
    expect(listUsage).toHaveBeenCalledTimes(2);
  });

  it("a board's switch to the usage screen is what triggers it", async () => {
    const farol = new Farol({ info() {}, warn() {} });
    const observer = { refreshUsageSoon: vi.fn() } as unknown as Observer;
    const note = (screen?: string) =>
      (farol as unknown as { noteScreen(o: Observer, d: string, m: object): void }).noteScreen(observer, "b", { screen });
    note("home");
    note("usage");
    note("usage");
    note(undefined);
    note("deck");
    note("usage");
    expect(observer.refreshUsageSoon).toHaveBeenCalledTimes(2);
    await farol.close();
  });
});
