// End to end against the isolated dev daemon (dev/dev-daemon.sh up && install):
// a fake board (dev/board.mjs) polls and acts exactly like paseo.rs while a
// driver creates scripted agents through the SDK. Prints a transcript and
// exits non-zero on the first failed check.
//
//   FAROL_URL=ws://127.0.0.1:47661/ws FAROL_PASSWORD=farol-dev FAROL_WORKDIR=<empty dir> node dev/e2e.mjs
import { createPaseoClient } from "@getpaseo/client";
import { FakeBoard } from "./board.mjs";

const URL_ = process.env.FAROL_URL ?? "ws://127.0.0.1:47661/ws";
const PASSWORD = process.env.FAROL_PASSWORD ?? "farol-dev";
const WORKDIR = process.env.FAROL_WORKDIR;
if (!WORKDIR) throw new Error("set FAROL_WORKDIR to an empty scratch directory (agents' cwd)");
const SKIP_IDLE = process.env.FAROL_SKIP_IDLE === "1";

const t0 = Date.now();
const stamp = () => `${((Date.now() - t0) / 1000).toFixed(1).padStart(6)}s`;
const say = (...a) => console.log(stamp(), ...a);
let failures = 0;
function check(ok, what, extra = "") {
  say(ok ? "PASS" : "FAIL", what, extra);
  if (!ok) failures += 1;
  return ok;
}

// ---------------------------------------------------------------- the board
const board = new FakeBoard({ url: URL_, password: PASSWORD, device: "e2e-board" });
let latest = null;
const snapshots = [];
const waiters = [];
let polling = true;

function brief(s) {
  return s.agents
    .map((a) => {
      let x = `${a.title ?? a.id}:${a.state}`;
      if (a.now) x += ` now="${a.now}"`;
      if (a.todo) x += ` todo=${a.todo.done}/${a.todo.total}`;
      if (a.ask) x += ` ask=${a.ask.kind}[${a.ask.options.join("|")}]`;
      if (a.stoppable) x += " stoppable";
      return x;
    })
    .join("; ");
}

async function pollLoop() {
  while (polling) {
    const r = await board.next();
    if (r.error) {
      say("poll error", r.error);
      await new Promise((res) => setTimeout(res, 2000));
      continue;
    }
    const s = r.output;
    if (s.idle) {
      snapshots.push({ at: Date.now(), idle: true, ms: r.ms });
      continue;
    }
    latest = s;
    snapshots.push({ at: Date.now(), rev: s.rev, ms: r.ms });
    say(`snapshot rev=${s.rev} (${r.ms} ms)`, brief(s));
    for (const w of [...waiters]) {
      if (w.pred(s)) {
        waiters.splice(waiters.indexOf(w), 1);
        w.resolve(s);
      }
    }
  }
}

function until(what, pred, ms = 20_000) {
  if (latest && pred(latest)) return Promise.resolve(latest);
  return new Promise((resolve) => {
    const w = { pred, resolve };
    waiters.push(w);
    setTimeout(() => {
      if (waiters.includes(w)) {
        waiters.splice(waiters.indexOf(w), 1);
        check(false, `timed out waiting: ${what}`);
        resolve(latest);
      }
    }, ms);
  });
}

const agentIn = (s, id) => s?.agents.find((a) => a.id === id);

// ---------------------------------------------------------------- the driver
const driver = createPaseoClient({ url: URL_, password: PASSWORD, clientId: "farol-e2e-driver" });
await driver.connect();

async function spawn(title, prompt) {
  const handle = await driver.agents.create({
    config: { provider: "scripted/script" },
    cwd: WORKDIR,
    title,
    prompt,
  });
  say(`driver created ${title} (${handle.id.slice(0, 8)}) prompt="${prompt}"`);
  return handle.id;
}

async function scriptedInputs() {
  const ws = new FakeBoard({ url: URL_, password: PASSWORD, device: "e2e-inspector" });
  await ws.connect();
  const r = await new Promise((resolve) => {
    ws.nextId += 1;
    const requestId = `i${ws.nextId}`;
    ws.pending.set(requestId, resolve);
    ws.ws.send(
      JSON.stringify({
        type: "session",
        message: { type: "plugin.rpc.invoke.request", requestId, pluginId: "scripted-agents", method: "scripted.received", input: {} },
      }),
    );
  });
  ws.close();
  return r.output?.inputs ?? [];
}

async function timelineRows(agentId) {
  const page = await driver.agents.ref(agentId).timeline.refetch({ direction: "tail", limit: 50 });
  return page.entries.map((e) => e.item).filter((i) => i.type === "plugin" && i.pluginId === "farol");
}

// ---------------------------------------------------------------- scenario
try {
  // Wrong password: the daemon refuses the hello.
  const intruder = new FakeBoard({ url: URL_, password: "not-the-password", device: "intruder" });
  const refused = await intruder.connect().then(
    () => null,
    (e) => e.message,
  );
  intruder.close();
  check(refused?.includes("incorrect_password"), "wrong password is refused", refused ?? "");

  const info = await board.connect();
  check(info.version === "0.14.0", "board hello accepted", `daemon ${info.version}, host "${info.hostname}"`);
  void pollLoop();
  const first = await until("first snapshot", (s) => s.v === 1);
  check(first.readOnly === false, "first snapshot", `rev=${first.rev} tz=${first.tz} agents=${first.agents.length}`);

  check(first.settings != null, "snapshot carries Paseo's board settings", JSON.stringify(first.settings));

  // 0. The board's control centre switches sound off: the change stays on that
  // board (Paseo's shared settings do not move) and the plugin's board list shows it.
  const flipped = !first.settings.sound;
  const sent = await board.setPrefs({ sound: flipped, haptic: true, brightness: 70, dnd: false });
  check(sent.output?.ok === true, "farol.focus with prefs ok");
  const status = await board.rpc("farol.status", {});
  const me = status.output?.boards?.find((b) => b.device === board.device);
  check(me?.prefs?.brightness === 70 && me?.prefs?.sound === flipped, "recent boards show the board's prefs", JSON.stringify(me));
  await new Promise((r) => setTimeout(r, 1500));
  const shared = latest?.settings ?? first.settings;
  check(shared.sound === first.settings.sound, "Paseo's shared settings ignore one board's prefs", JSON.stringify(shared));
  await board.setPrefs({ sound: first.settings.sound, haptic: true, brightness: 70, dnd: false });

  check(me?.fw === "1.0.0" && me?.screen === "home", "recent boards show firmware and screen", `fw=${me?.fw} screen=${me?.screen}`);

  // 1. A working agent: live detail for the focused card.
  const work = await spawn("Fix the spawn error", "work fast");
  await until("work agent working", (s) => agentIn(s, work)?.state === "work");
  const tFocus = Date.now();
  const focusReply = board.setFocus(work);
  const focused = await until(
    "focused detail (todo + recent)",
    (s) => agentIn(s, work)?.todo != null && (agentIn(s, work)?.recent?.length ?? 0) > 0,
    10_000,
  );
  const fr = await focusReply;
  check(fr?.output?.ok === true, "farol.focus answered while farol.next waited", `${fr?.ms} ms`);
  check(agentIn(focused, work)?.todo != null, "focused card carries todo and recent", `${Date.now() - tFocus} ms after focus`);
  const done = await until("work agent done", (s) => agentIn(s, work)?.state === "done", 30_000);
  const w = agentIn(done, work);
  check(Boolean(w?.body?.includes("regression test")), "finished card has reply preview and cost", `body="${w?.body}" cost=${w?.cost} ctx=${w?.ctx}`);

  check(Boolean(w?.model), "finished card has a model label", `model="${w?.model}"`);

  // 2. Mark read: the card leaves the board.
  const seen = await board.act(work, "seen");
  check(seen.output?.ok === true, "act seen ok", JSON.stringify(seen.output));
  await until("work agent gone", (s) => !agentIn(s, work), 5_000);
  check(!agentIn(latest, work), "a card marked read stays off the board");

  // 3. A question, answered from the board.
  const ask = await spawn("Pick a lamp style", "ask fast");
  const asking = await until("question pending", (s) => agentIn(s, ask)?.state === "call", 15_000);
  const q = agentIn(asking, ask)?.ask;
  check(q?.kind === "question" && q.options.length === 3, "question card", `text="${q?.text}" options=${q?.options.join(" | ")}`);
  board.setFocus(ask);
  const tAnswer = Date.now();
  const answered = await board.act(ask, "answer", q.requestId, 1);
  check(answered.output?.ok === true, "act answer ok", `${answered.ms} ms`);
  const again = await board.act(ask, "answer", q.requestId, 0);
  check(again.output?.reason === "already_resolved", "second answer is already_resolved", JSON.stringify(again.output));
  const afterAsk = await until("question agent done", (s) => agentIn(s, ask)?.state === "done", 15_000);
  check(
    Boolean(agentIn(afterAsk, ask)?.body?.includes("Plasma")),
    "fake agent received the chosen option",
    `body="${agentIn(afterAsk, ask)?.body}" (${Date.now() - tAnswer} ms)`,
  );
  const askRows = await timelineRows(ask);
  check(
    askRows.some((r) => r.kind === "farol-action" && r.data?.action === "answer" && r.data?.device === `farol-${board.device}`),
    "timeline row names the board",
    JSON.stringify(askRows.map((r) => r.data)),
  );

  // 4. Permission, denied from the board.
  const perm = await spawn("Clean the build", "permission fast");
  const permAsk = await until("permission pending", (s) => agentIn(s, perm)?.state === "call", 15_000);
  const pq = agentIn(permAsk, perm)?.ask;
  check(pq?.kind === "permission", "permission card", `text="${pq?.text}" options=${pq?.options.join(" | ")}`);
  const denied = await board.act(perm, "answer", pq.requestId, 1);
  check(denied.output?.ok === true, "act deny ok");
  const afterPerm = await until("permission agent done", (s) => agentIn(s, perm)?.state === "done", 15_000);
  check(Boolean(agentIn(afterPerm, perm)?.body?.includes("denied")), "fake agent got the denial", `body="${agentIn(afterPerm, perm)?.body}"`);

  // 5. Stop a long turn.
  const long = await spawn("Run the long suite", "long fast");
  const running = await until("long agent stoppable", (s) => agentIn(s, long)?.state === "work" && agentIn(s, long)?.stoppable, 15_000);
  check(Boolean(agentIn(running, long)?.stoppable), "long card is stoppable", `now="${agentIn(running, long)?.now}"`);
  const tStop = Date.now();
  const stopped = await board.act(long, "stop");
  check(stopped.output?.ok === true, "act stop ok", `${stopped.ms} ms ${JSON.stringify(stopped.output)}`);
  const afterStop = await until("long agent left work", (s) => agentIn(s, long) && agentIn(s, long).state !== "work", 15_000);
  check(agentIn(afterStop, long)?.state !== "work", "turn stopped", `state=${agentIn(afterStop, long)?.state} after ${Date.now() - tStop} ms`);
  const inputs = await scriptedInputs();
  check(inputs.some((i) => i.type === "session.interrupt"), "fake agent received session.interrupt");
  check(inputs.some((i) => i.type === "session.permission" && i.detail?.updatedInput?.answers), "fake agent received answers", JSON.stringify(inputs.find((i) => i.type === "session.permission")?.detail));
  const stopAgain = await board.act(long, "stop");
  check(stopAgain.output?.reason === "already_resolved", "stopping an idle agent is already_resolved", JSON.stringify(stopAgain.output));
  const longRows = await timelineRows(long);
  check(longRows.some((r) => r.data?.action === "stop"), "timeline row appended for the stop");

  // 6. A failed turn.
  const fail = await spawn("Build with Rsbuild", "fail fast");
  const failed = await until("fail agent failed", (s) => agentIn(s, fail)?.state === "fail", 15_000);
  check(Boolean(agentIn(failed, fail)?.error), "failed card", `error="${agentIn(failed, fail)?.error}" body="${agentIn(failed, fail)?.body}"`);

  // 7. Unknown agent.
  const missing = await board.act("no-such-agent", "answer", "x", 0);
  check(missing.output?.reason === "not_found", "unknown agent is not_found", JSON.stringify(missing.output));

  // 8. Push rate: never more than 5 full snapshots in any second.
  const full = snapshots.filter((s) => !s.idle).map((s) => s.at);
  let peak = 0;
  for (let i = 0; i < full.length; i++) {
    let j = i;
    while (j < full.length && full[j] - full[i] < 1000) j++;
    peak = Math.max(peak, j - i);
  }
  check(peak <= 5, "push rate at most 5 per second", `peak ${peak} in 1 s over ${full.length} snapshots`);

  // 9. Idle long poll: nothing changes, the poll answers idle within 20 s.
  if (!SKIP_IDLE) {
    board.setFocus(null);
    await new Promise((r) => setTimeout(r, 1500));
    const before = snapshots.length;
    const tIdle = Date.now();
    await new Promise((resolve) => {
      const iv = setInterval(() => {
        if (snapshots.slice(before).some((s) => s.idle)) {
          clearInterval(iv);
          resolve();
        }
      }, 200);
      setTimeout(() => {
        clearInterval(iv);
        resolve();
      }, 24_000);
    });
    const idle = snapshots.slice(before).find((s) => s.idle);
    check(Boolean(idle) && idle.ms >= 19_000 && idle.ms <= 21_000, "idle answer after ~20 s", `poll took ${idle?.ms} ms, waited ${Date.now() - tIdle} ms`);
  }

  // 10. The board turns to its usage screen: usage is fetched at once, but at
  // most once a minute, so wait out the fetch made when observation started.
  {
    const status = async () => (await board.rpc("farol.status", {})).output;
    const before = (await status()).usageAt;
    const wait = before + 61_000 - Date.now();
    if (wait > 0) {
      say(`waiting ${Math.round(wait / 1000)} s for the once-a-minute usage limit`);
      await new Promise((r) => setTimeout(r, wait));
    }
    const tUsage = Date.now();
    await board.setFocus(null, "usage");
    const after = (await status()).usageAt;
    check(after >= tUsage, "usage screen refreshes usage at once", `fetched ${after - tUsage} ms after the switch`);
    await board.setFocus(null, "deck");
    await board.setFocus(null, "usage");
    check((await status()).usageAt === after, "a second switch within a minute does not refetch");
    check(latest.usage.length > 0, "snapshot carries usage", latest.usage.map((u) => `${u.label} ${u.used}%`).join(", ") + ` fuel=${JSON.stringify(latest.fuel)}`);
  }
} catch (error) {
  check(false, "scenario threw", String(error?.stack ?? error));
} finally {
  polling = false;
  board.close();
  await driver.close().catch(() => {});
  say(failures ? `${failures} check(s) failed` : "all checks passed");
  process.exit(failures ? 1 : 0);
}
