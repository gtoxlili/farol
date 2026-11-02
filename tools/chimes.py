#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Synthesize Farol's three chimes as raw PCM for components/app_feedback.

Deterministic, standard library only, no samples: each note is a sine with a
quiet octave partial, a short attack and an exponential decay, like a small
bell. Output: <out>/chime_{call,done,fail}.pcm, signed 16-bit little-endian,
mono, SAMPLE_RATE Hz (app_feedback opens the codec with the same format).

    python tools/chimes.py --out <directory>
"""

import argparse
import math
import struct
from pathlib import Path

SAMPLE_RATE = 16000
PEAK = 0.55  # of full scale; the codec volume sets the loudness
ATTACK_S = 0.006
OCTAVE_PARTIAL = 0.18
TAIL_S = 0.02  # silence after the last note, so the codec drains cleanly


def note(freq_hz: float, length_s: float, decay_s: float, bend_to_hz: float | None = None) -> list[float]:
    count = round(length_s * SAMPLE_RATE)
    out = []
    phase = 0.0
    for i in range(count):
        t = i / SAMPLE_RATE
        freq = freq_hz if bend_to_hz is None else freq_hz + (bend_to_hz - freq_hz) * i / count
        phase += 2.0 * math.pi * freq / SAMPLE_RATE
        envelope = min(1.0, t / ATTACK_S) * math.exp(-t / decay_s)
        # The last 8 ms fade to zero: no click when the note is cut.
        envelope *= min(1.0, (count - i) / (0.008 * SAMPLE_RATE))
        out.append(envelope * (math.sin(phase) + OCTAVE_PARTIAL * math.sin(2.0 * phase)))
    return out


def silence(length_s: float) -> list[float]:
    return [0.0] * round(length_s * SAMPLE_RATE)


def to_pcm(samples: list[float]) -> bytes:
    peak = max(abs(s) for s in samples) or 1.0
    scale = PEAK * 32767 / peak
    return b"".join(struct.pack("<h", round(s * scale)) for s in samples + silence(TAIL_S))


CHIMES = {
    # Someone is waiting: two rising notes (E5, A5).
    "call": lambda: note(659.25, 0.13, 0.09) + silence(0.03) + note(880.0, 0.28, 0.12),
    # A turn finished: one soft note (G5).
    "done": lambda: note(783.99, 0.26, 0.10),
    # A turn failed: one low note, sagging slightly (E4 to D4).
    "fail": lambda: note(329.63, 0.36, 0.16, bend_to_hz=293.66),
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", type=Path, required=True, help="directory for chime_*.pcm")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    for name, make in CHIMES.items():
        (args.out / f"chime_{name}.pcm").write_bytes(to_pcm(make()))


if __name__ == "__main__":
    main()
