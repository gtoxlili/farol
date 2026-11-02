#!/usr/bin/env python3
"""Development shortcuts for Farol, run from anywhere inside the ESP-Mosaico workspace.

    python tools/dev.py scene              regenerate ui/main.json, scene.rs and the Rust GSP bindings
    python tools/dev.py sim                run the simulator with the demo fleet (browser preview)
    python tools/dev.py sim --welcome      start on the first-boot page instead of the demo
    python tools/dev.py check [SCENARIO]   run a scripted simulator check (default: the full suite)

`sim` and `check` pass the dynamic Chinese font the board uses; `mosaico.py project sim`
does not, so runtime Chinese text would render as boxes there.
"""
from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

APP = Path(__file__).resolve().parents[1]
FONT = APP / "components/app_logic/ui/fonts/NotoSansSC-Farol.ttf"
HOST = APP / "components/app_logic/host"
CHECKS = HOST / "checks"


def workspace() -> Path:
    for parent in APP.parents:
        if (parent / "mosaico.py").exists():
            return parent
    raise SystemExit("run inside an ESP-Mosaico workspace (mosaico.py not found above the app)")


def run(cmd: list[str], **kw) -> int:
    print("+", " ".join(str(c) for c in cmd), file=sys.stderr)
    return subprocess.call(cmd, **kw)


def scene(_: argparse.Namespace) -> int:
    if run([sys.executable, str(APP / "tools/ui/gen_scene.py")]):
        return 1
    ws = workspace()
    build = ws / "submodule/esp-mosaico-utils/mosaico-tools/skills/idf-low-noise-build/scripts/idf_low_noise_build.py"
    # The IDF build regenerates the scene headers first; it may then fail on stale
    # bindings, which the next step fixes.
    run([sys.executable, str(build), "--project", str(APP), "build"], cwd=ws)
    return run(["cargo", "xtask", "gen-gsp"], cwd=APP / "rust")


def sim_command(extra: list[str]) -> list[str]:
    ws = workspace()
    gsp = APP / "managed_components/espressif__esp-gsp"
    if not gsp.exists():
        raise SystemExit("run one IDF build first (python tools/dev.py scene) to install espressif/esp-gsp")
    # fetch_gspc.py picks the tool versions from the installed component.
    os.environ["ESP_GSP_COMPONENT_DIR"] = str(gsp)
    fetch = ws / "submodule/esp-mosaico-utils/mosaico-tools/tools/gsp-sim/fetch_gspc.py"
    gspc = subprocess.check_output([sys.executable, str(fetch)], text=True).strip()
    host = subprocess.check_output([sys.executable, str(fetch), "--sim"], text=True).strip()
    return [sys.executable, str(gsp / "tools/sim_bridge/run.py"), "--project", str(HOST),
            "--build-dir", str(APP / "build-sim"), "--gspc", gspc, "--host", host,
            "--dynamic-font", str(FONT), *extra]


def sim(args: argparse.Namespace) -> int:
    if args.welcome:
        os.environ["FAROL_SIM"] = "welcome"
    return run(sim_command([]), cwd=workspace())


def check(args: argparse.Namespace) -> int:
    scenarios = [Path(args.scenario)] if args.scenario else sorted(CHECKS.glob("*.json"))
    out = Path(args.out or APP / "build-sim/checks")
    failed = 0
    for scenario in scenarios:
        report = out / scenario.stem
        if report.exists():
            for f in report.iterdir():
                f.unlink()
            report.rmdir()
        mode = "welcome" if scenario.stem.startswith("welcome") else "demo"
        os.environ["FAROL_SIM"] = mode
        code = run(sim_command(["--scenario", str(scenario.resolve()), "--report-dir", str(report)]),
                   cwd=workspace())
        print(f"{'PASS' if code == 0 else 'FAIL'} {scenario.stem} -> {report}", file=sys.stderr)
        failed += code != 0
    return 1 if failed else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    sub.add_parser("scene").set_defaults(fn=scene)
    p = sub.add_parser("sim")
    p.add_argument("--welcome", action="store_true")
    p.set_defaults(fn=sim)
    p = sub.add_parser("check")
    p.add_argument("scenario", nargs="?")
    p.add_argument("--out")
    p.set_defaults(fn=check)
    args = parser.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
