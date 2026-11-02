#!/usr/bin/env python3
"""Farol's icon: the lamp from the board's home page, a halo whose light pools below.

Writes docs/media/icon.svg and renders plugin/assets/icon.png with rsvg-convert.
"""
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SVG = ROOT / "docs/media/icon.svg"
PNG = ROOT / "plugin/assets/icon.png"
PNG_SIZE = 512

SIZE = 1024
CORNER = 228
C = SIZE / 2
RADIUS = 288
STROKE = 46
# The spark that sits just outside the ring, as on the board.
SPARK = (C + 214, C + 232)

ICON = f"""<svg xmlns="http://www.w3.org/2000/svg" width="{SIZE}" height="{SIZE}" viewBox="0 0 {SIZE} {SIZE}">
  <defs>
    <radialGradient id="tile" cx="50%" cy="44%" r="72%">
      <stop offset="0" stop-color="#1D1813"/>
      <stop offset="1" stop-color="#080706"/>
    </radialGradient>
    <linearGradient id="ring" x1="0" y1="{C - RADIUS}" x2="0" y2="{C + RADIUS}" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#9A5C17"/>
      <stop offset="0.45" stop-color="#E5942C"/>
      <stop offset="0.8" stop-color="#FFC157"/>
      <stop offset="1" stop-color="#FFE7B5"/>
    </linearGradient>
    <radialGradient id="inside" cx="50%" cy="50%" r="50%">
      <stop offset="0.55" stop-color="#FFB13B" stop-opacity="0"/>
      <stop offset="1" stop-color="#FFB13B" stop-opacity="0.10"/>
    </radialGradient>
    <radialGradient id="spark" cx="50%" cy="50%" r="50%">
      <stop offset="0" stop-color="#FFF6E2"/>
      <stop offset="0.3" stop-color="#FFD48A" stop-opacity="0.7"/>
      <stop offset="1" stop-color="#FFB13B" stop-opacity="0"/>
    </radialGradient>
    <filter id="soft" x="-30%" y="-30%" width="160%" height="160%"><feGaussianBlur stdDeviation="18"/></filter>
    <filter id="wide" x="-40%" y="-40%" width="180%" height="180%"><feGaussianBlur stdDeviation="56"/></filter>
    <filter id="pool" x="-60%" y="-200%" width="220%" height="500%"><feGaussianBlur stdDeviation="34"/></filter>
    <clipPath id="clip"><rect width="{SIZE}" height="{SIZE}" rx="{CORNER}"/></clipPath>
  </defs>
  <g clip-path="url(#clip)">
    <rect width="{SIZE}" height="{SIZE}" fill="url(#tile)"/>
    <circle cx="{C}" cy="{C}" r="{RADIUS}" fill="url(#inside)"/>
    <circle cx="{C}" cy="{C}" r="{RADIUS}" fill="none" stroke="url(#ring)" stroke-width="{STROKE * 2.2}" filter="url(#wide)" opacity="0.6"/>
    <circle cx="{C}" cy="{C}" r="{RADIUS}" fill="none" stroke="url(#ring)" stroke-width="{STROKE * 1.3}" filter="url(#soft)" opacity="0.85"/>
    <ellipse cx="{C}" cy="{C + RADIUS + 6}" rx="200" ry="44" fill="#FFB13B" opacity="0.75" filter="url(#pool)"/>
    <ellipse cx="{C}" cy="{C + RADIUS - 2}" rx="120" ry="22" fill="#FFE2A6" opacity="0.8" filter="url(#soft)"/>
    <circle cx="{C}" cy="{C}" r="{RADIUS}" fill="none" stroke="url(#ring)" stroke-width="{STROKE}"/>
    <circle cx="{SPARK[0]}" cy="{SPARK[1]}" r="46" fill="url(#spark)"/>
    <circle cx="{SPARK[0]}" cy="{SPARK[1]}" r="9" fill="#FFF8EA"/>
  </g>
  <rect x="1" y="1" width="{SIZE - 2}" height="{SIZE - 2}" rx="{CORNER - 1}" fill="none" stroke="#FFFFFF" stroke-opacity="0.07" stroke-width="2"/>
</svg>
"""


def main():
    SVG.write_text(ICON)
    subprocess.run(["rsvg-convert", "-w", str(PNG_SIZE), "-h", str(PNG_SIZE), "-o", str(PNG), str(SVG)], check=True)
    print(SVG.relative_to(ROOT), PNG.relative_to(ROOT))


if __name__ == "__main__":
    main()
