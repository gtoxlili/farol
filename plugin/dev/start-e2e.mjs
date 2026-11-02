// End to end for sessions started on a board, against the isolated dev daemon
// (dev/dev-daemon.sh up && install, with an agent profile "scripted" on the
// scripted provider under daemon.agentProfiles in its config.json): the app's
// language reaches the board, the default workspace shows as `compose`, and
// farol.start creates a scripted agent there that the board's next snapshot
// carries. Exits non-zero on the first failed check.
//
//   FAROL_DEV_DIR=<dev dir> FAROL_WORKDIR=<empty dir> node dev/start-e2e.mjs
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { createPaseoClient } from "@getpaseo/client";
import { FakeBoard } from "./board.mjs";

const URL_ = process.env.FAROL_URL ?? "ws://127.0.0.1:47661/ws";
const PASSWORD = process.env.FAROL_PASSWORD ?? "farol-dev";
const DEV_DIR = process.env.FAROL_DEV_DIR;
const WORKDIR = process.env.FAROL_WORKDIR;
if (!DEV_DIR || !WORKDIR) throw new Error("set FAROL_DEV_DIR and FAROL_WORKDIR");

let failures = 0;
function check(ok, what, extra = "") {
  console.log(ok ? "PASS" : "FAIL", what, extra);
  if (!ok) failures += 1;
  return ok;
}

const driver = createPaseoClient({ url: URL_, password: PASSWORD, clientId: "farol-start-e2e" });
await driver.connect();
const workspace = await driver.workspaces.open(WORKDIR);
check(Boolean(workspace.id), "the scratch directory opens as a workspace", workspace.id);

// The plugin's settings, as the settings page would save them; then a reload
// so the plugin reads them.
const settingsDir = `${DEV_DIR}/paseo-home/plugin-settings/farol`;
mkdirSync(settingsDir, { recursive: true });
writeFileSync(
  `${settingsDir}/farol.json`,
  JSON.stringify({ version: 1, values: { workspaceId: workspace.id, profileId: "scripted" } }),
);
execFileSync(new URL("./dev-daemon.sh", import.meta.url).pathname, ["reload", "farol"], {
  env: { ...process.env, FAROL_DEV_DIR: DEV_DIR },
  stdio: "ignore",
});

const board = new FakeBoard({ url: URL_, password: PASSWORD, device: "start-e2e" });
await board.connect();
const language = await board.rpc("farol.language", { locale: "en-US" });
check(language.output?.ok === true, "an app reports its language", JSON.stringify(language));

let snapshot = (await board.next()).output;
check(snapshot?.lang === "en", "the board's snapshot carries the app's language", snapshot?.lang);
check(Boolean(snapshot?.compose?.workspace), "and where new sessions go", JSON.stringify(snapshot?.compose));

const started = await board.rpc("farol.start", { device: "farol-start-e2e", text: "hello from the board" });
const agentId = started.output?.agent;
check(started.output?.ok === true && Boolean(agentId), "farol.start creates an agent", JSON.stringify(started));

let seen = false;
for (let i = 0; i < 10 && !seen; i += 1) {
  snapshot = (await board.next()).output ?? snapshot;
  seen = snapshot.agents?.some((a) => a.id === agentId) ?? false;
}
check(seen, "the board's next snapshot carries the new agent");

if (agentId) {
  const agent = driver.agents.ref(agentId);
  await agent.refresh();
  const current = agent.current();
  check(current?.labels?.["farol.device"] === "farol-start-e2e", "the agent carries the board's label", JSON.stringify(current?.labels));
  check(current?.workspaceId === workspace.id, "in the default workspace", current?.workspaceId);
  await driver.agents.archive?.(agentId).catch?.(() => undefined);
}

board.close?.();
await driver.close?.();
console.log(failures ? `${failures} failed` : "all passed");
process.exit(failures ? 1 : 0);
