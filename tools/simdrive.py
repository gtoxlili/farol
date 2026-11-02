#!/usr/bin/env python3
"""Drive Farol in the GSP simulator with real pointer input and save screenshots.

The scenario format of sim_bridge has taps and drags only; Farol's actions are
long presses and holds, so this drives the simulator's API channel directly
(`feed_pointer`, `wait`, `get_property`, `screenshot`).

    python tools/simdrive.py              run every flow
    python tools/simdrive.py answer stop  run some flows
Screenshots go to build-sim/drive/<flow>/. Exit status is non-zero when a check fails.
"""
from __future__ import annotations

import os
import socket
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import dev  # noqa: E402

APP = dev.APP
OUT = APP / "build-sim/drive"
PORT = 18766
FPS = 60


class Sim:
    def __init__(self, flow: str, env: dict[str, str]):
        self.dir = OUT / flow
        self.dir.mkdir(parents=True, exist_ok=True)
        for f in self.dir.glob("*.png"):
            f.unlink()
        environ = {**os.environ, **env}
        os.environ.update(env)
        cmd = dev.sim_command(["--headless", "--api-port", str(PORT), "--duration", "0"])
        self.log = open(self.dir / "sim.log", "w")
        self.proc = subprocess.Popen(cmd, cwd=dev.workspace(), env=environ, stdout=self.log,
                                     stderr=subprocess.STDOUT)
        sys.path.insert(0, str(APP / "managed_components/espressif__esp-gsp/tools/sim_bridge"))
        import rpc_client  # noqa: PLC0415

        deadline = time.time() + 240
        while True:
            try:
                socket.create_connection(("127.0.0.1", PORT), timeout=1).close()
                break
            except OSError:
                if time.time() > deadline or self.proc.poll() is not None:
                    raise SystemExit(f"simulator did not start; see {self.dir / 'sim.log'}")
                time.sleep(0.5)
        self._ctx = rpc_client.connect(port=PORT)
        self.peer = self._ctx.__enter__()
        self.failures: list[str] = []
        self.frames(30)

    def close(self) -> None:
        try:
            self.peer.call("quit")
        except Exception:  # noqa: BLE001 - the simulator may already be gone
            pass
        try:
            self._ctx.__exit__(None, None, None)
        except Exception:  # noqa: BLE001
            pass
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            self.proc.wait(timeout=10)
        self.log.close()

    def call(self, method: str, params: dict | None = None):
        reply = self.peer.call(method, params or {})
        if "error" in reply:
            raise RuntimeError(f"{method}: {reply['error']}")
        return reply.get("result")

    def frames(self, n: int) -> None:
        self.call("wait", {"frames": n})

    def ms(self, ms: int) -> None:
        self.frames(max(1, ms * FPS // 1000))

    def press(self, x: int, y: int, hold_ms: int) -> None:
        self.call("feed_pointer", {"x": x, "y": y, "pressed": True})
        self.ms(hold_ms)
        self.call("feed_pointer", {"x": x, "y": y, "pressed": False})
        self.frames(6)

    def drag(self, x1: int, y1: int, x2: int, y2: int) -> None:
        self.call("drag", {"x1": x1, "y1": y1, "x2": x2, "y2": y2})
        self.frames(30)

    def tap(self, x: int, y: int) -> None:
        self.call("tap", {"x": x, "y": y})
        self.frames(12)

    def shot(self, name: str) -> None:
        self.call("screenshot", {"path": str(self.dir / f"{name}.png"), "format": "png"})

    def visible(self, name: str) -> bool:
        """Visible on screen: the object and every parent group."""
        return self.call("inspect_object", {"name": name})["effective_visible"] is True

    def page(self, name: str) -> int:
        return int(self.call("component_get_motion", {"name": name})["state"]["value"])

    def key(self, *keys: str) -> None:
        """Taps on-screen keyboard keys by name, e.g. "lower_key_0_1"."""
        for k in keys:
            self.call("tap_object", {"name": f"kb_{k}"})
            self.frames(6)

    def expect(self, what: str, ok: bool) -> None:
        print(f"  {'ok  ' if ok else 'FAIL'} {what}")
        if not ok:
            self.failures.append(what)


def option_y(j: int) -> int:
    return 186 + 92 + j * 46 + 20


def flow_home(s: Sim) -> None:
    s.shot("1-home")
    s.drag(420, 300, 60, 300)
    s.expect("swipe left reaches the deck", s.page("screens") == 2)
    s.shot("2-deck-first")
    s.drag(240, 380, 240, 100)
    s.shot("3-deck-second")
    s.drag(240, 380, 240, 100)
    s.shot("4-deck-third")
    s.drag(60, 300, 420, 300)
    s.drag(60, 300, 420, 300)
    s.expect("two swipes right reach usage", s.page("screens") == 0)
    s.shot("5-usage")
    s.drag(420, 300, 60, 300)
    s.drag(240, 6, 240, 380)
    s.frames(30)
    s.shot("6-control")


def flow_answer(s: Sim) -> None:
    s.shot("1-home-call")
    s.expect("the lamp asks for attention", s.visible("home_word_call"))
    s.tap(240, 206)
    s.frames(30)
    s.expect("tapping the lamp opens the deck", s.page("screens") == 2)
    s.shot("2-question")
    s.call("feed_pointer", {"x": 238, "y": option_y(0), "pressed": True})
    s.ms(250)
    s.shot("3-pressing")
    s.ms(500)
    s.call("feed_pointer", {"x": 238, "y": option_y(0), "pressed": False})
    s.frames(20)
    s.shot("4-sent")
    s.ms(2600)
    s.shot("5-working-again")


def to_running(s: Sim) -> None:
    """Open the deck on the first running agent: three seconds into the demo
    the deck holds a failed, a finished and then the running cards."""
    s.drag(420, 300, 60, 300)
    s.drag(240, 380, 240, 100)
    s.drag(240, 380, 240, 100)
    s.expect("the third card is the running agent", s.visible("c2_run") or s.visible("c0_run")
             or s.visible("c1_run"))


def flow_stop(s: Sim) -> None:
    to_running(s)
    s.shot("1-running")
    s.call("feed_pointer", {"x": 240, "y": 420, "pressed": True})
    s.ms(900)
    s.shot("2-holding")
    s.expect("holding shows the stop ring", s.visible("ov_hold"))
    s.ms(800)
    s.call("feed_pointer", {"x": 240, "y": 420, "pressed": False})
    s.frames(30)
    s.shot("3-stopped")
    s.expect("the ring closes after the stop", not s.visible("ov_hold"))


def flow_cancel(s: Sim) -> None:
    to_running(s)
    s.call("feed_pointer", {"x": 240, "y": 420, "pressed": True})
    s.ms(600)
    s.call("feed_pointer", {"x": 240, "y": 420, "pressed": False})
    s.frames(20)
    s.expect("letting go early cancels the stop", not s.visible("ov_hold"))
    s.shot("1-still-running")


def flow_welcome(s: Sim) -> None:
    s.shot("1-welcome")
    s.expect("first boot shows the welcome page", s.visible("st_welcome"))
    s.tap(138, 406)
    s.frames(20)
    s.expect("scan opens the camera page", s.visible("st_scan"))
    s.shot("2-scan")


# Lower-case letters on the keyboard's first layer (row_column).
LETTERS = {"a": "1_0", "b": "2_5", "c": "2_3", "g": "1_4", "n": "2_6", "o": "0_8", "r": "0_3",
           "w": "0_1", "x": "2_2"}
OK_KEY = "lower_key_3_2"


def type_word(s: Sim, word: str) -> None:
    s.key(*(f"lower_key_{LETTERS[ch]}" for ch in word))


def flow_direct(s: Sim) -> None:
    s.tap(342, 406)
    s.frames(20)
    s.expect("直接连接 opens the keyboard page", s.visible("st_input"))
    type_word(s, "abc")
    s.shot("1-host")
    s.key(OK_KEY)
    s.frames(10)
    s.shot("2-port")
    s.key(OK_KEY)
    s.frames(20)
    s.expect("an empty port takes 6767 and leads to the SSL choice", s.visible("st_tls"))
    s.shot("3-ssl")
    s.tap(240, 158)
    s.frames(20)
    s.expect("the SSL choice asks for the password", s.visible("st_input"))
    type_word(s, "x")
    s.shot("4-password")
    s.key(OK_KEY)
    s.frames(20)
    s.expect("four steps lead to the confirm page", s.visible("st_pair"))
    s.shot("5-confirm")


def flow_wifi(s: Sim) -> None:
    s.drag(240, 6, 240, 380)
    s.frames(20)
    s.tap(120, 115)
    s.frames(20)
    s.expect("the Wi-Fi tile opens network details", s.visible("st_netinfo"))
    s.shot("1-details")
    s.tap(138, 406)
    s.frames(20)
    s.expect("choose a network lists the scan", s.visible("st_wifi") and s.visible("wf_row1"))
    s.shot("2-list")
    s.tap(240, 175)
    s.frames(20)
    s.expect("a protected network asks for its password", s.visible("st_input"))
    type_word(s, "wrong")
    s.key(OK_KEY)
    s.frames(20)
    s.expect("a wrong password stays on the page", s.visible("st_input"))
    s.shot("3-wrong-password")


def flow_link(s: Sim) -> None:
    s.drag(240, 6, 240, 380)
    s.frames(20)
    s.tap(350, 115)
    s.frames(20)
    s.expect("the Paseo tile opens connection details", s.visible("st_link"))
    s.shot("1-link")
    s.tap(342, 406)
    s.frames(10)
    s.shot("2-forget-armed")
    s.tap(138, 406)
    s.frames(20)
    s.expect("重新连接 offers Paseo's two ways", s.visible("st_connect"))
    s.shot("3-connect")


def flow_compose(s: Sim) -> None:
    s.press(240, 206, 700)
    s.frames(10)
    s.expect("holding the lamp opens a new session", s.visible("st_compose"))
    s.shot("1-compose")
    s.call("feed_pointer", {"x": 240, "y": 348, "pressed": True})
    s.ms(800)
    s.expect("holding the button listens", s.visible("cp_listen"))
    s.shot("2-listening")
    s.call("feed_pointer", {"x": 240, "y": 348, "pressed": False})
    s.ms(1400)
    s.expect("letting go shows the transcript and Send", s.visible("cp_send"))
    s.shot("3-transcript")
    s.tap(356, 440)
    s.frames(30)
    s.expect("sending opens the new session's card", s.page("screens") == 2)
    s.shot("4-new-card")


def flow_english(s: Sim) -> None:
    s.shot("1-home")
    s.drag(420, 300, 60, 300)
    s.shot("2-deck")
    s.drag(240, 6, 240, 380)
    s.frames(20)
    s.shot("3-control")
    s.expect("the control centre opens", s.page("screens") == 2)


def flow_welcome_en(s: Sim) -> None:
    s.shot("1-welcome")
    s.expect("first boot shows the welcome page", s.visible("st_welcome"))
    s.tap(414, 60)
    s.frames(12)
    s.shot("2-switched")


def flow_pulse(s: Sim) -> None:
    # The demo's "runtime" agent finishes 70 s into the script; this run starts at 69 s.
    seen = False
    for _ in range(60):
        if s.visible("lamp_pulse"):
            seen = True
            break
        s.ms(50)
    s.expect("a finished turn plays the pulse", seen)
    s.ms(250)
    s.shot("1-pulse")
    s.ms(1000)
    s.expect("the pulse plays once", not s.visible("lamp_pulse"))


FLOWS = {
    "home": (flow_home, {"FAROL_SIM": "demo", "FAROL_SIM_T": "3000"}),
    "answer": (flow_answer, {"FAROL_SIM": "demo", "FAROL_SIM_T": "13000"}),
    "stop": (flow_stop, {"FAROL_SIM": "demo", "FAROL_SIM_T": "3000"}),
    "cancel": (flow_cancel, {"FAROL_SIM": "demo", "FAROL_SIM_T": "3000"}),
    "welcome": (flow_welcome, {"FAROL_SIM": "welcome"}),
    "direct": (flow_direct, {"FAROL_SIM": "welcome"}),
    "wifi": (flow_wifi, {"FAROL_SIM": "demo", "FAROL_SIM_T": "3000"}),
    "link": (flow_link, {"FAROL_SIM": "demo", "FAROL_SIM_T": "3000"}),
    "pulse": (flow_pulse, {"FAROL_SIM": "demo", "FAROL_SIM_T": "69000"}),
    "compose": (flow_compose, {"FAROL_SIM": "demo", "FAROL_SIM_T": "3000"}),
    "english": (flow_english, {"FAROL_SIM": "demo", "FAROL_SIM_T": "13000", "FAROL_SIM_LANG": "en"}),
    "welcome_en": (flow_welcome_en, {"FAROL_SIM": "welcome", "FAROL_SIM_LANG": "en"}),
}


def main() -> int:
    names = sys.argv[1:] or list(FLOWS)
    failed = []
    for name in names:
        fn, env = FLOWS[name]
        print(f"{name}:")
        sim = Sim(name, env)
        try:
            fn(sim)
        finally:
            sim.close()
        failed += [f"{name}: {f}" for f in sim.failures]
    print("all flows passed" if not failed else "failed:\n  " + "\n  ".join(failed))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
