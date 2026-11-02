#!/usr/bin/env python3
"""Regenerate the screenshots and clips in docs/media from the simulator.

The simulator runs the board's scene and Rust logic on the built-in sample
agents (rust/logic/src/demo.rs), once in Chinese and once in English:

    python tools/media.py            screenshots and clips
    python tools/media.py shots      screenshots only

Clips need ffmpeg; the overview needs ImageMagick. Output goes to
docs/media/<zh|en>/: <name>.png, and <clip>.gif (shown in the READMEs) with
<clip>.mp4 next to it. The English overview and clips are also copied to
plugin/assets for the plugin's store listing.
"""
from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import simdrive  # noqa: E402
from simdrive import Sim, option_y, to_running  # noqa: E402

MEDIA = simdrive.APP / "docs/media"
LANGS = ("zh", "en")
CLIP_FPS = 20
LAMP = (240, 206)
MIC = (240, 224)  # the compose page's lamp
SEND = (240, 404)  # the hold-to-send bar
HOLD_TO_STOP = (240, 420)


class Clip:
    """A clip: a screenshot every few simulator frames, then ffmpeg."""

    def __init__(self, sim: Sim, name: str):
        self.sim = sim
        self.frames_dir = sim.dir / f"clip-{name}"
        if self.frames_dir.exists():
            shutil.rmtree(self.frames_dir)
        self.frames_dir.mkdir(parents=True)
        self.name = name
        self.n = 0
        self.step = simdrive.FPS // CLIP_FPS

    def run(self, ms: int) -> None:
        for _ in range(max(1, ms * CLIP_FPS // 1000)):
            self.sim.call("wait", {"frames": self.step})
            self.sim.call("screenshot", {"path": str(self.frames_dir / f"{self.n:04}.png"),
                                         "format": "png"})
            self.n += 1

    def pointer(self, xy: tuple[int, int], pressed: bool) -> None:
        self.sim.call("feed_pointer", {"x": xy[0], "y": xy[1], "pressed": pressed})

    def save(self, out: Path) -> None:
        frames = str(self.frames_dir / "%04d.png")
        rate = ["-framerate", str(CLIP_FPS), "-i", frames]
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", *rate, "-c:v", "libx264",
                        "-pix_fmt", "yuv420p", "-crf", "22", "-movflags", "+faststart",
                        str(out / f"{self.name}.mp4")], check=True)
        palette = ("scale=360:-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];"
                   "[b][p]paletteuse=dither=bayer:bayer_scale=4")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", *rate, "-vf", palette,
                        str(out / f"{self.name}.gif")], check=True)


def shoot(sim: Sim, out: Path, name: str) -> None:
    sim.call("screenshot", {"path": str(out / f"{name}.png"), "format": "png"})


def session(lang: str, env: dict[str, str], run, out: Path, clips: bool) -> None:
    sim = Sim(f"media-{lang}-{run.__name__}", {**env, "FAROL_SIM_LANG": lang})
    try:
        run(sim, out, clips)
    finally:
        sim.close()


def asking(sim: Sim, out: Path, clips: bool) -> None:
    """A question arrived: the amber lamp, the card, an answer by holding."""
    if not clips:
        shoot(sim, out, "home-call")
        sim.tap(*LAMP)
        sim.frames(30)
        shoot(sim, out, "question")
        return
    clip = Clip(sim, "answer")
    clip.run(1200)
    sim.call("tap", {"x": LAMP[0], "y": LAMP[1]})
    clip.run(900)
    clip.pointer((238, option_y(0)), True)
    clip.run(900)
    clip.pointer((238, option_y(0)), False)
    clip.run(2400)
    clip.save(out)


def working(sim: Sim, out: Path, clips: bool) -> None:
    """The deck three seconds in: a failed, a finished and a running card."""
    if not clips:
        shoot(sim, out, "home")
        sim.drag(60, 300, 420, 300)
        shoot(sim, out, "usage")
        sim.drag(420, 300, 60, 300)
        sim.drag(420, 300, 60, 300)
        shoot(sim, out, "failed")
        sim.drag(240, 380, 240, 100)
        shoot(sim, out, "done")
        sim.drag(240, 380, 240, 100)
        shoot(sim, out, "running")
        sim.drag(240, 6, 240, 380)
        sim.frames(20)
        shoot(sim, out, "control")
        return
    to_running(sim)
    clip = Clip(sim, "stop")
    clip.run(700)
    clip.pointer(HOLD_TO_STOP, True)
    clip.run(2300)
    clip.pointer(HOLD_TO_STOP, False)
    clip.run(1500)
    clip.save(out)


def voice(sim: Sim, out: Path, clips: bool) -> None:
    """Hold the lamp, hold it again to talk, hold the bar: a new session's card."""
    clip = Clip(sim, "voice") if clips else None
    run = clip.run if clip else sim.ms

    def hold(xy: tuple[int, int], ms: int, shot: str | None = None, at: int = 0) -> None:
        sim.call("feed_pointer", {"x": xy[0], "y": xy[1], "pressed": True})
        if shot and not clip:
            run(at)
            shoot(sim, out, shot)
            run(ms - at)
        else:
            run(ms)
        sim.call("feed_pointer", {"x": xy[0], "y": xy[1], "pressed": False})

    run(600)
    hold(LAMP, 700)
    run(800)
    if not clip:
        shoot(sim, out, "compose")
    hold(MIC, 1800, "listening", 1000)
    run(300)
    if not clip:
        shoot(sim, out, "transcribing")
    run(1300)
    if not clip:
        shoot(sim, out, "transcript")
    hold(SEND, 700, "holding", 300)
    run(1600)
    if clip:
        clip.save(out)
    else:
        shoot(sim, out, "new-session")


def welcome(sim: Sim, out: Path, clips: bool) -> None:
    if not clips:
        shoot(sim, out, "welcome")


def overview(out: Path) -> None:
    """Four screens side by side for the top of the README."""
    names = ["home-call", "question", "running", "transcript"]
    subprocess.run(["magick", *(str(out / f"{n}.png") for n in names), "-bordercolor", "#000000",
                    "-border", "12", "+append", "-resize", "1600x", str(out / "overview.png")],
                   check=True)


def listing(out: Path) -> None:
    """The plugin's store listing (plugin/paseo-plugin.json `media`) ships its own
    copies. plugin-settings.png is a Paseo screenshot, taken by hand."""
    assets = simdrive.APP / "plugin/assets"
    for name in ("overview.png", "plugin-settings.png", "voice.mp4", "answer.mp4"):
        if (out / name).exists():
            shutil.copy2(out / name, assets / name)


def main() -> int:
    clips = sys.argv[1:] != ["shots"]
    runs = [
        (asking, {"FAROL_SIM": "demo", "FAROL_SIM_T": "12500"}),
        (working, {"FAROL_SIM": "demo", "FAROL_SIM_T": "3000"}),
        (voice, {"FAROL_SIM": "demo", "FAROL_SIM_T": "3000"}),
        (welcome, {"FAROL_SIM": "welcome"}),
    ]
    for lang in LANGS:
        out = MEDIA / lang
        out.mkdir(parents=True, exist_ok=True)
        for run, env in runs:
            print(f"{lang}: {run.__name__}")
            session(lang, env, run, out, clips=False)
            if clips and run is not welcome:
                session(lang, env, run, out, clips=True)
        overview(out)
    listing(MEDIA / "en")
    return 0


if __name__ == "__main__":
    sys.exit(main())
