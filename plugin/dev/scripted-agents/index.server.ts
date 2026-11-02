import type { PluginServerContext } from "@getpaseo/plugin/server";
import { defineRpc } from "@getpaseo/plugin";
import { z } from "zod";
import { received, scriptedProvider } from "./server/provider";

/** What the fake agents were sent (prompts, answers, interrupts), for end-to-end checks. */
const receivedRpc = defineRpc({
  name: "scripted.received",
  input: z.object({}),
  output: z.object({
    inputs: z.array(
      z.object({ at: z.number(), type: z.string(), sessionId: z.string().optional(), detail: z.unknown() }),
    ),
  }),
});

export default function contribute(server: PluginServerContext) {
  server.registerProvider(scriptedProvider());
  server.registerUsageSource({
    id: "scripted-usage",
    label: "Scripted",
    input: z.object({}),
    discover: async (scope) => (scope.kind === "global" ? [{ key: "scripted", label: "Scripted", input: {} }] : []),
    fetch: async () => ({
      status: "available",
      planLabel: "Dev",
      windows: [
        { id: "5h", label: "5h", summary: true, usedPct: 42, resetsAt: new Date(Date.now() + 3 * 3600e3).toISOString() },
        { id: "wk", label: "week", summary: true, usedPct: 18, resetsAt: new Date(Date.now() + 4 * 86400e3).toISOString() },
      ],
    }),
  });
  server.handle(receivedRpc, () => ({ inputs: received.map((r) => ({ ...r, detail: r.detail ?? null })) }));
  return () => {};
}
