#!/usr/bin/env python3
"""Generate Farol's GSP scene (components/app_logic/ui/main.json).

The scene is authored here, in one place, so the design tokens, the geometry
and the names the Rust logic addresses stay consistent. The output is
committed; the build reads the JSON. After editing, run

    python3 tools/ui/gen_scene.py

then rebuild and regenerate the Rust bindings (`cargo xtask gen-gsp`).

Layout of the scene (paint order = list order, later objects on top):
  screens       horizontal PageFlow: usage | home | deck
    deck/cards  vertical cyclic PageFlow of three recycled agent cards
  ov_status     full-screen status pages (welcome, no Wi-Fi, no Paseo, connect, pair,
                scan, SSL choice, keyboard input, Wi-Fi and Paseo details)
  ov_hold       hold-to-stop overlay
  banner        "Paseo unreachable" strip under the status bar
  cc            control centre Drawer from the top edge
  sb_*          status bar, painted last so it stays above everything
"""
from __future__ import annotations

import json
import struct
import subprocess
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "components/app_logic/ui/main.json"

W = H = 480
BAR_H = 36  # status bar height; content starts below it

# ---------------------------------------------------------------- tokens
BG = "#000000"
INK1 = "#F2EFE8"   # primary text, warm white
INK2 = "#8C8A84"   # secondary text
INK3 = "#4E4D49"   # tertiary text
INK4 = "#33322F"   # faint text
LINE = "#1C1C1A"   # hairline
SURF = "#0E0E0D"   # raised surface (buttons)
EDGE = "#2A2926"   # button border
TILE = "#171716"   # control centre tiles
DIM = "#2E2D2A"    # inactive glyphs
WORK = "#6FA8FF"   # moonlight: working
CALL = "#FFB13B"   # lamplight: needs you
DONE = "#F2EFE8"   # finished, unread
FAIL = "#FF5A4E"   # ember: failed
SEEN = "#3A3936"   # finished, seen
VOICE = "#A99BFF"  # the lamp while you talk
VOICE_DEEP = "#5B4FD0"  # the send bar filling
VOICE_DIM = "#3A3358"
CLEAR = "#00000000"
VIEWFINDER = "#141516"  # the scan page's empty viewfinder

PREVIEW = 360  # camera preview side, px (app_camera's APP_CAMERA_PREVIEW_SIDE)
SCAN_PREVIEW_PNG = "images/scan_preview.png"

NOTO = "fonts/NotoSansSC-Farol.ttf"
G_THIN = "fonts/Geist-Thin-Farol.ttf"
G_XL = "fonts/Geist-ExtraLight-Farol.ttf"
G_L = "fonts/Geist-Light-Farol.ttf"
G_R = "fonts/Geist-Regular-Farol.ttf"
G_M = "fonts/Geist-Medium-Farol.ttf"
MONO = "fonts/GeistMono-Regular-Farol.ttf"

DIGITS = "0123456789"
CARDS = 3          # recycled card pages in the deck
OPTIONS = 4        # answer options per card
FLEET_DOTS = 8     # status dots on the home page and the deck rail
USAGE_CELLS = 4
WIFI_ROWS = 6      # networks listed on the "choose a network" page
SAVED_ROWS = 4     # saved networks listed on the network details page
DRIFT = 3          # burn-in drift room, px each way (the logic moves +-2)

objects: list[dict] = []


class Tr:
    """Text in both languages: the label is bound, and the logic writes the
    language in use from scene.rs `TEXTS` (Chinese is the scene's default)."""

    def __init__(self, zh: str, en: str):
        self.zh, self.en = zh, en


def tr(zh: str, en: str) -> Tr:
    return Tr(zh, en)


# (bind name, Chinese, English) of every translated label, in scene order.
texts: list[tuple[str, str, str]] = []


def add(obj: dict) -> dict:
    objects.append(obj)
    return obj


def under(parent: str | None, obj: dict) -> dict:
    if parent is None:
        obj["parent"] = -1
    else:
        obj["parent_name"] = parent
    return add(obj)


def label(parent, x, y, w, h, text="", *, font=NOTO, size=18, color=INK1, align="left",
          name=None, bind=False, charset=None, ellipsis=False, hidden=False, dyn_y=None,
          input=False):
    if isinstance(text, Tr):
        if bind:
            raise ValueError("a translated label is bound through TEXTS")
        name = name or f"tx{len(texts)}"
        texts.append((name, text.zh, text.en))
        text, bind = text.zh, True
    obj = {"type": "label", "x": x, "y": y, "w": w, "h": h, "text": text, "font": font,
           "font_size": size, "fg_color": color, "text_align": align}
    if input:
        obj["input"] = True
    if name:
        obj["name"] = name
    if bind:
        obj["bind"] = name
    if charset:
        obj["font_charset"] = charset
    if ellipsis:
        obj["overflow"] = "ellipsis"
    if hidden:
        obj["hidden"] = True
    if dyn_y is not None:
        obj["y"] = {"default": y, "min": dyn_y[0], "max": dyn_y[1]}
    return under(parent, obj)


def rect(parent, x, y, w, h, color, *, radius=0, name=None, border=None, border_w=1,
         visible_bind=False, color_bind=False, hidden=False, dyn_x=None, dyn_y=None, dyn_w=None,
         opacity=None):
    obj = {"type": "rect", "x": x, "y": y, "w": w, "h": h, "bg_color": color, "radius": radius}
    if name:
        obj["name"] = name
    if border:
        obj["border_color"] = border
        obj["border_width"] = border_w
    if dyn_x is not None:
        obj["x"] = {"default": x, "min": dyn_x[0], "max": dyn_x[1]}
    if dyn_y is not None:
        obj["y"] = {"default": y, "min": dyn_y[0], "max": dyn_y[1]}
    if dyn_w is not None:
        obj["w"] = {"default": w, "min": dyn_w[0], "max": dyn_w[1]}
    if opacity is not None:
        obj["opacity"] = opacity
    if hidden:
        obj["hidden"] = True
    under(parent, obj)
    if visible_bind:
        visibility(obj)
    if color_bind:
        obj["bind"] = obj["name"]
        obj["bind_target"] = "color"
    return obj


def solid_png(rel: str, w: int, h: int, color: str) -> str:
    """Write an opaque single-color RGB PNG under ui/ (stdlib only) and return
    its scene path. Canvas placeholders must be static opaque bitmaps."""
    rgb = bytes.fromhex(color.lstrip("#"))
    raw = b"".join(b"\x00" + rgb * w for _ in range(h))

    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))
    path = OUT.parent / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists() or path.read_bytes() != png:
        path.write_bytes(png)
    return rel


def visibility(obj: dict) -> None:
    """Give a named object a runtime visibility bind (GSP_BIND_<NAME>_VIS)."""
    if "bind" in obj:
        raise ValueError(f"{obj['name']} already has a bind")
    obj["bind"] = obj["name"] + "_vis"
    obj["bind_target"] = "visible"


def group(parent, name, x, y, w, h, *, hidden=False, vis=True, bg=None, drift=False):
    obj = {"type": "container", "name": name, "x": x, "y": y, "w": w, "h": h}
    if drift:
        # Burn-in guard: the logic slides the whole group a pixel or two each minute.
        obj["x"] = {"default": x, "min": x - DRIFT, "max": x + DRIFT}
        obj["y"] = {"default": y, "min": y - DRIFT, "max": y + DRIFT}
    if bg:
        obj["bg_color"] = bg
    if hidden:
        obj["hidden"] = True
    under(parent, obj)
    if vis:
        visibility(obj)
    return obj


def button(parent, name, x, y, w, h, *, text="", font=NOTO, size=18, bg=SURF, fg=INK1,
           border=EDGE, radius=None, callback=None, events=None, pressed_dim=48, hidden=False):
    obj = {"type": "button", "name": name, "x": x, "y": y, "w": w, "h": h, "bg_color": bg,
           "fg_color": fg, "radius": h // 2 if radius is None else radius, "pressed_dim": pressed_dim}
    translated = text if isinstance(text, Tr) else None
    if text and not translated:
        obj.update({"text": text, "font": font, "font_size": size})
    if border:
        obj["border_color"] = border
        obj["border_width"] = 1
    if callback:
        obj["callback"] = callback
    if events:
        obj["events"] = events
    if hidden:
        obj["hidden"] = True
    under(parent, obj)
    if translated:
        # A button's own text has no bind: a centred child label carries it.
        line = round(size * 1.45)
        label(name, 0, (h - line) // 2, w, line, translated, font=font, size=size, color=fg,
              align="center")
    return obj


def color_variants(parent, name, x, y, w, h, colors: dict[str, str], **kw):
    """A label whose text and color both change at runtime.

    GSP gives a label one bind, so each color is its own container (visibility
    bind ``<name>_<key>_vis``) holding a text-bound label ``<name>_<key>_t``. The
    logic writes the text to the visible variant and switches visibility.
    """
    for key, color in colors.items():
        v = f"{name}_{key}"
        group(parent, v, x, y, w, h, hidden=True)
        label(v, 0, 0, w, h, "", color=color, name=f"{v}_t", bind=True, **kw)


STATE_COLORS = {"call": CALL, "work": WORK, "done": DONE, "fail": FAIL, "dim": INK2}


def calls(**events) -> list[dict]:
    return [{"event": event, "action": "call", "callback": cb} for event, cb in events.items()]


# ---------------------------------------------------------------- pages

def usage_page(parent):
    label(parent, 28, 44, 200, 26, tr("额度", "Usage"), size=18, color=INK2)
    label(parent, 252, 46, 200, 22, "", size=14, color=INK3, align="right", name="usage_updated",
          bind=True, ellipsis=True)
    cells = [(36, 80), (252, 80), (36, 274), (252, 274)]
    for i, (x, y) in enumerate(cells):
        cell = f"u{i}"
        group(parent, cell, x, y, 192, 184)
        # An Arc's bind slot carries its value, so each color is its own Arc.
        for tone, fg in (("work", WORK), ("call", CALL), ("fail", FAIL)):
            arc = under(cell, {"type": "arc", "name": f"{cell}_arc_{tone}", "x": 26, "y": 0, "w": 140,
                               "h": 140, "min": 0, "max": 100, "value": 0, "start_angle": 225,
                               "sweep": 270, "thickness": 5, "bg_color": "#1A1A18", "fg_color": fg,
                               "hidden": tone != "work"})
            visibility(arc)
        label(cell, 26, 42, 140, 52, "0", font=G_XL, size=42, align="center", name=f"{cell}_pct",
              bind=True, charset=DIGITS)
        label(cell, 26, 92, 140, 20, "%", font=G_L, size=14, color=INK3, align="center")
        label(cell, 0, 138, 192, 24, "", size=15, align="center", name=f"{cell}_name", bind=True,
              ellipsis=True)
        label(cell, 0, 160, 192, 22, "", size=13, color=INK3, align="center", name=f"{cell}_reset",
              bind=True, ellipsis=True)
    # A translated label's bind is its text, so a container carries the visibility.
    group(parent, "usage_empty", 40, 220, 400, 30, hidden=True)
    label("usage_empty", 0, 0, 400, 30, tr("还没有额度数据", "No usage data yet"), size=17, color=INK3, align="center")


def home_page(parent):
    group(parent, "home_body", 0, 0, W, H, vis=False, drift=True)
    parent = "home_body"
    add_orb(parent, "lamp", 90, 56, 300, color=CALL, value=35, particles=2)
    # A finished turn: one white bloom over the lamp, played once by the logic
    # (behind the orb it only showed faintly through the lamp's centre).
    pulse = under(parent, {"type": "effect", "name": "lamp_pulse", "x": 40, "y": 6, "w": 400,
                           "h": 400, "effect": "pulse", "fg_color": DONE, "playing": False,
                           "period_ms": 900, "strength": 100, "softness": 85, "hidden": True})
    visibility(pulse)
    # Tap: the most urgent card. Hold: a new session.
    # (GSP: a hold fires `long` and no `click`; `callback` would replace `events`.)
    button(parent, "lamp_btn", 120, 86, 240, 240, bg=CLEAR, border=None, pressed_dim=0,
           events=calls(click="lamp_tap", long="lamp_long"))
    group(parent, "home_count_box", 90, 118, 300, 120)
    label("home_count_box", 0, 0, 300, 120, "0", font=G_XL, size=112, align="center",
          name="home_count", bind=True, charset=DIGITS)
    group(parent, "home_time_box", 90, 140, 300, 100, hidden=True)
    label("home_time_box", 0, 0, 300, 100, "00:00", font=G_THIN, size=76, align="center",
          name="home_time", bind=True, charset=DIGITS + ":-")
    color_variants(parent, "home_word", 90, 236, 300, 32, STATE_COLORS, size=22, align="center")
    label(parent, 40, 372, 400, 22, "", font=MONO, size=15, color=INK2, align="center",
          name="home_who", bind=True, ellipsis=True)
    label(parent, 40, 396, 400, 32, "", size=22, align="center", name="home_what", bind=True,
          ellipsis=True)
    label(parent, 40, 428, 400, 18, "", size=13, color=INK3, align="center", name="home_hint",
          bind=True, ellipsis=True)
    for i in range(FLEET_DOTS):
        # The color bind takes the object's bind slot, so the logic hides unused
        # dots by moving them off-screen (x = W) instead of a visibility bind.
        rect(parent, W, 459, 7, 7, SEEN, radius=4, name=f"home_dot{i}", dyn_x=(0, W),
             color_bind=True)


def add_orb(parent, name, x, y, d, *, color, value, particles, strength=85, softness=90):
    return under(parent, {"type": "charging_orb", "name": name, "x": x, "y": y, "w": d, "h": d,
                          "style": "halo", "value": value, "fg_color": color, "strength": strength,
                          "period_ms": 4200, "particles": particles, "softness": softness,
                          "ripple": 55, "charging": True, "runtime_style": True})


def card_page(parent, i):
    c = f"c{i}"
    # Full-card press target for hold-to-stop; painted first so controls stay on top.
    button(parent, f"{c}_hold", 0, BAR_H, 456, H - BAR_H, bg=CLEAR, border=None, radius=0,
           pressed_dim=0, events=calls(press="hold_press", release="hold_release",
                                       cancel="hold_cancel"))
    rect(parent, 28, 53, 8, 8, WORK, radius=4, name=f"{c}_dot", color_bind=True)
    color_variants(parent, f"{c}_word", 44, 45, 320, 24, STATE_COLORS, size=17)
    label(parent, 28, 72, 420, 22, "", size=15, color=INK2, name=f"{c}_meta", bind=True,
          ellipsis=True)
    label(parent, 28, 96, 420, 86, "", size=26, name=f"{c}_title", bind=True)
    rect(parent, 28, 186, 420, 1, LINE)

    # question / permission / plan
    q = f"{c}_q"
    group(parent, q, 0, 186, 456, 294, hidden=True)
    label(q, 28, 12, 400, 68, "", size=19, name=f"{q}_text", bind=True)
    for j in range(OPTIONS):
        o = f"{c}_o{j}"
        btn = button(q, o, 28, 92 + j * 46, 420, 40, bg=SURF, border=EDGE, radius=14, pressed_dim=0,
                     events=calls(press=f"opt{j}_press", long=f"opt{j}_long",
                                  release=f"opt{j}_release", cancel=f"opt{j}_cancel"))
        visibility(btn)
        fill = group(o, f"{o}_fill", 0, 0, 420, 40, hidden=True, bg=CALL)
        fill["radius"] = 14
        label(f"{o}_fill", 48, 6, 270, 28, "", size=18, color=BG, name=f"{o}_ft", bind=True,
              ellipsis=True)
        label(f"{o}_fill", 322, 8, 84, 24, tr("已发送", "Sent"), size=15, color=BG, align="right")
        ring = rect(o, 0, 0, 420, 40, CLEAR, radius=14, name=f"{o}_ring", border=CALL, border_w=2,
                    hidden=True)
        visibility(ring)
        key = label(o, 16, 8, 30, 24, "ABCD"[j], font=G_M, size=17, color=INK2, name=f"{o}_k")
        key["bind"] = f"{o}_k_color"
        key["bind_target"] = "color"
        label(o, 48, 6, 300, 28, "", size=18, name=f"{o}_t", bind=True, ellipsis=True)
    label(q, 28, 278, 420, 16, "", size=13, color=INK3, align="center", name=f"{q}_hint",
          bind=True)

    # running
    r = f"{c}_run"
    group(parent, r, 0, 186, 456, 294, hidden=True)
    label(r, 28, -2, 260, 84, "00:00", font=G_THIN, size=68, name=f"{r}_timer", bind=True,
          charset=DIGITS + ":-")
    label(r, 328, 44, 120, 24, tr("本轮", "This turn"), size=15, color=INK3, align="right")
    label(r, 28, 88, 420, 28, "", font=MONO, size=18, name=f"{r}_now", bind=True, ellipsis=True)
    alive = under(r, {"type": "effect", "name": f"{r}_alive", "x": 28, "y": 120, "w": 420, "h": 6,
                      "effect": "shimmer", "fg_color": WORK, "bg_color": "#0F1626",
                      "period_ms": 1800})
    visibility(alive)
    for k, color in enumerate((INK2, INK3, INK4)):
        label(r, 28, 134 + k * 22, 420, 22, "", font=MONO, size=14, color=color,
              name=f"{r}_a{k}", bind=True, ellipsis=True)
    under(r, {"type": "progress", "name": f"{r}_bar", "x": 28, "y": 216, "w": 420, "h": 4,
              "value": 0, "bg_color": "#1A1A18", "fg_color": WORK, "radius": 2})
    label(r, 28, 228, 420, 24, "", size=16, color=INK2, name=f"{r}_todo", bind=True, ellipsis=True)
    label(r, 28, 264, 420, 22, "", size=14, color=INK3, align="center", name=f"{r}_hint",
          bind=True)

    # done / failed
    d = f"{c}_done"
    group(parent, d, 0, 186, 456, 294, hidden=True)
    label(d, 28, 14, 400, 26, "", font=MONO, size=17, color=FAIL, name=f"{d}_err", bind=True,
          ellipsis=True)
    label(d, 28, 12, 420, 190, "", size=19, name=f"{d}_body", bind=True, dyn_y=(0, 120))
    label(d, 28, 228, 300, 22, "", font=MONO, size=14, color=INK3, name=f"{d}_cost", bind=True,
          ellipsis=True)
    button(d, f"{d}_seen", 348, 218, 100, 40, text=tr("已读", "Mark read"), size=16, radius=20, callback="seen")


def deck_page(parent):
    under(parent, {"type": "page_flow", "name": "cards", "x": 0, "y": 0, "w": W, "h": H,
                   "page_count": CARDS, "selected": 0, "axis": "vertical", "cyclic": True,
                   "bar_height": 0, "pages": [f"card{i}" for i in range(CARDS)]})
    for i in range(CARDS):
        under("cards", {"type": "layer", "name": f"card{i}", "x": 0, "y": H * i, "w": W, "h": H,
                        "bg_color": BG})
        card_page(f"card{i}", i)
    for i in range(FLEET_DOTS):
        rect(parent, 464, H, 6, 6, SEEN, radius=3, name=f"rail{i}", dyn_y=(0, H), color_bind=True)
    rect(parent, 461, 237, 12, 12, CLEAR, radius=6, name="rail_cursor", border=INK1, border_w=2,
         dyn_y=(0, H))
    # A translated label's bind is its text, so a container carries the visibility.
    group(parent, "deck_empty", 40, 220, 400, 30, hidden=True)
    label("deck_empty", 0, 0, 400, 30, tr("还没有 agent", "No agents yet"), size=17, color=INK3, align="center")


# ---------------------------------------------------------------- overlays

def wifi_glyph(parent, prefix, cy):
    for k, d in enumerate((220, 150, 80)):
        for lit in (False, True):
            arc = under(parent, {"type": "arc", "x": 240 - d // 2, "y": cy - d // 2, "w": d, "h": d,
                                 "min": 0, "max": 100, "value": 100, "start_angle": 315,
                                 "sweep": 90, "thickness": 10, "bg_color": DIM,
                                 "fg_color": INK2 if lit else DIM})
            if lit:
                arc["name"] = f"{prefix}_lit{k}"
                arc["hidden"] = True
                visibility(arc)
    rect(parent, 231, cy - 9, 18, 18, FAIL, radius=9, name=f"{prefix}_dot", color_bind=True)


def status_pages():
    layer = {"type": "layer", "name": "ov_status", "x": 0, "y": 0, "w": W, "h": H, "bg_color": BG,
             "hidden": True}
    under(None, layer)
    visibility(layer)
    p = "ov_status"

    g = "st_welcome"
    group(p, g, 0, 0, W, H, hidden=True)
    add_orb(g, "st_welcome_orb", 120, 70, 240, color="#55534E", value=0, particles=0,
            strength=35, softness=95)
    label(g, 120, 166, 240, 40, "Farol", font=G_XL, size=34, align="center")
    label(g, 120, 206, 240, 22, "", font=MONO, size=14, color=INK3, align="center",
          name="st_welcome_name", bind=True)
    # The board's language before Paseo says which (then it follows Paseo).
    button(g, "st_lang", 372, 44, 84, 32, bg=CLEAR, fg=INK2, border=EDGE, radius=16,
           pressed_dim=24, callback="lang_toggle")
    label("st_lang", 0, 5, 84, 22, "", size=14, color=INK2, align="center", name="st_lang_t",
          bind=True)
    label(g, 28, 318, 424, 24, tr("配对码在 Paseo 的 设置 › 你的 Host › 配对设备", "Pairing code: Paseo › Settings › your host › Pair device"), size=15,
          color=INK2, align="center")
    label(g, 28, 342, 424, 22, tr("本地网络或 VPN 里也可以直接连接", "On a local network or VPN, connect directly"), size=14, color=INK3,
          align="center")
    button(g, "st_welcome_scan", 40, 380, 196, 52, text=tr("扫描二维码", "Scan QR code"), size=18, bg=INK1, fg=BG,
           border=None, callback="scan_open")
    button(g, "st_welcome_direct", 244, 380, 196, 52, text=tr("直接连接", "Direct connection"), size=18,
           callback="direct_open")

    g = "st_nowifi"
    group(p, g, 0, 0, W, H, hidden=True)
    wifi_glyph(g, "st_nowifi", 214)
    label(g, 28, 252, 424, 36, tr("没有连上 Wi-Fi", "No Wi-Fi"), size=26, align="center")
    label(g, 28, 292, 424, 26, "", size=17, color=INK2, align="center", name="st_nowifi_reason",
          bind=True, ellipsis=True)
    label(g, 28, 318, 424, 22, "", size=14, color=INK3, align="center", name="st_nowifi_saved",
          bind=True, ellipsis=True)
    button(g, "st_nowifi_scan", 40, 362, 196, 52, text=tr("扫码加入", "Scan to join"), size=18, bg=INK1, fg=BG,
           border=None, callback="scan_open")
    button(g, "st_nowifi_pick", 244, 362, 196, 52, text=tr("选择网络", "Choose network"), size=18, callback="wifi_pick")
    label(g, 28, 432, 424, 22, tr("手机的 Wi-Fi 设置里可以出示网络二维码", "Your phone's Wi-Fi settings can show a network code"), size=14, color=INK3,
          align="center")

    g = "st_nopaseo"
    group(p, g, 0, 0, W, H, hidden=True)
    add_orb(g, "st_nopaseo_orb", 150, 62, 180, color="#55534E", value=0, particles=0,
            strength=35, softness=95)
    rect(g, 233, 145, 14, 14, FAIL, radius=7)
    label(g, 28, 258, 424, 36, tr("连不上 Paseo", "Can't reach Paseo"), size=26, align="center")
    label(g, 28, 298, 424, 24, "", size=15, color=FAIL, align="center", name="st_np_reason",
          bind=True, ellipsis=True)
    label(g, 28, 324, 424, 22, "", size=15, color=INK2, align="center", name="st_np_retry",
          bind=True, ellipsis=True)
    button(g, "st_np_retry_btn", 40, 362, 196, 52, text=tr("立即重试", "Retry now"), size=18, bg=INK1, fg=BG,
           border=None, callback="retry")
    button(g, "st_np_repair", 244, 362, 196, 52, text=tr("重新连接", "Reconnect"), size=18,
           callback="connect_open")
    label(g, 28, 432, 424, 22, "", size=14, color=INK3, align="center", name="st_np_foot",
          bind=True, ellipsis=True)

    g = "st_pair"
    group(p, g, 0, 0, W, H, hidden=True)
    rect(g, 165, 117, 22, 22, INK1, radius=11)
    rect(g, 196, 127, 88, 2, "#3A3936")
    rect(g, 293, 117, 22, 22, CALL, radius=11)
    label(g, 120, 150, 112, 22, "Farol", font=MONO, size=13, color=INK3, align="center")
    label(g, 248, 150, 112, 22, "Paseo", font=MONO, size=13, color=INK3, align="center")
    label(g, 28, 196, 424, 34, "", size=25, align="center", name="st_pair_title", bind=True,
          ellipsis=True)
    label(g, 28, 238, 424, 24, "", font=MONO, size=16, color=INK2, align="center",
          name="st_pair_url", bind=True, ellipsis=True)
    label(g, 28, 264, 424, 22, "", size=14, color=INK3, align="center", name="st_pair_code",
          bind=True, ellipsis=True)
    label(g, 28, 300, 424, 24, tr("连接后，Paseo 的 Farol 设置里会出现这块板子。", "Once connected, the board shows in Paseo's Farol settings."), size=15,
          color=INK2, align="center")
    label(g, 28, 324, 424, 24, tr("要断开，就在板子的控制中心里忘记这台 Paseo。", "To disconnect, forget this Paseo in the control centre."), size=15,
          color=INK2, align="center")
    button(g, "st_pair_cancel", 40, 380, 196, 52, text=tr("取消", "Cancel"), size=18, callback="pair_cancel")
    button(g, "st_pair_connect", 244, 380, 196, 52, text=tr("连接", "Connect"), size=18, bg=INK1, fg=BG,
           border=None, callback="pair_connect")

    g = "st_scan"
    group(p, g, 0, 0, W, H, hidden=True)
    # The camera preview: app_camera pushes 360x360 RGB565 frames into this
    # image (GSP Canvas, bind scan_preview) while the page is open; otherwise
    # the placeholder shows the empty viewfinder. Unscaled and opaque, as
    # Canvas requires; the corner marks, sweep and note paint above it.
    placeholder = solid_png(SCAN_PREVIEW_PNG, PREVIEW, PREVIEW, VIEWFINDER)
    under(g, {"type": "image", "name": "scan_preview", "x": 60, "y": 74, "w": PREVIEW,
              "h": PREVIEW, "image": placeholder, "codec": "raw", "bind": "scan_preview",
              "canvas": "push"})
    for x, y, w, h in [(60, 74, 40, 4), (60, 74, 4, 40), (380, 74, 40, 4), (416, 74, 4, 40),
                       (60, 430, 40, 4), (60, 394, 4, 40), (380, 430, 40, 4), (416, 394, 4, 40)]:
        rect(g, x, y, w, h, INK1, radius=2)
    under(g, {"type": "effect", "name": "scan_sweep", "x": 64, "y": 250, "w": 352, "h": 4,
              "effect": "shimmer", "fg_color": CALL, "bg_color": "#26282C", "period_ms": 1400})
    label(g, 28, 42, 424, 24, tr("对准 Paseo 的配对二维码，或手机分享的 Wi-Fi 码", "Point at Paseo's pairing code or a phone's Wi-Fi code"), size=15,
          color=INK2, align="center")
    label(g, 60, 220, 360, 60, "", size=15, color=INK2, align="center", name="scan_note",
          bind=True)
    button(g, "scan_direct", 52, 440, 140, 36, text=tr("直接连接", "Direct connection"), size=16, bg=CLEAR, border=None,
           callback="direct_open")
    button(g, "scan_cancel", 300, 440, 128, 36, text=tr("取消", "Cancel"), size=16, bg=CLEAR, fg=INK2,
           border=None, callback="scan_cancel")

    connect_page(p)
    tls_page(p)
    compose_page(p)
    input_page(p)
    wifi_page(p)
    net_details_page(p)
    link_details_page(p)


CANCEL = tr("取消", "Cancel")
BACK = tr("返回", "Back")


def page_header(g, prefix, title, close_text, close_callback, *, bind_title=False):
    label(g, 24, 44, 330, 30, title, size=22, name=f"{prefix}_title" if bind_title else None,
          bind=bind_title, ellipsis=True)
    button(g, f"{prefix}_close", 372, 40, 84, 36, text=close_text, size=16, bg=CLEAR, fg=INK2,
           border=None, pressed_dim=24, callback=close_callback)


def choice(g, name, y, title, hint, callback, *, primary=False):
    """A wide two-line button: what it does, and when to pick it."""
    button(g, name, 24, y, 432, 92, bg=INK1 if primary else TILE, border=None, radius=20,
           pressed_dim=32, callback=callback)
    label(name, 24, 18, 384, 30, title, size=22, color=BG if primary else INK1)
    label(name, 24, 52, 384, 22, hint, size=15, color=INK3 if primary else INK2)


def connect_page(p):
    """The two ways the Paseo app offers to add a host (添加连接)."""
    g = "st_connect"
    group(p, g, 0, 0, W, H, hidden=True)
    page_header(g, "cn", tr("连接 Paseo", "Connect Paseo"), CANCEL, "cn_cancel")
    choice(g, "cn_scan", 104, tr("扫描二维码", "Scan QR code"),
           tr("加密 relay 连接。", "Encrypted relay connection."), "scan_open", primary=True)
    choice(g, "cn_direct", 210, tr("直接连接", "Direct connection"),
           tr("本地网络或 VPN。", "Local network or VPN."), "direct_open")
    label(g, 28, 330, 424, 24, tr("配对码在 Paseo 的 设置 › 你的 Host › 配对设备", "Pairing code: Paseo › Settings › your host › Pair device"), size=15,
          color=INK2, align="center")
    label(g, 28, 356, 424, 22, tr("第一次显示时，Paseo 会先问你是否启用中继", "The first time, Paseo asks to turn on its relay"), size=14, color=INK3,
          align="center")


def tls_page(p):
    """Direct connection, step 3 of 4: SSL or not, as the app's 使用 SSL switch."""
    g = "st_tls"
    group(p, g, 0, 0, W, H, hidden=True)
    page_header(g, "tl", tr("直接连接", "Direct connection"), CANCEL, "tl_cancel")
    label(g, 24, 80, 432, 20, "", size=13, color=INK3, name="tl_caption", bind=True,
          ellipsis=True)
    choice(g, "tl_plain", 112, tr("不使用 SSL", "No SSL"),
           tr("本地网络或 VPN 里直接连 daemon", "Straight to the daemon on a LAN or VPN"), "tls_off",
           primary=True)
    choice(g, "tl_secure", 218, tr("使用 SSL", "Use SSL"),
           tr("daemon 前面有 HTTPS 反向代理", "Behind an HTTPS reverse proxy"), "tls_on")
    label(g, 28, 336, 424, 22, tr("使用 SSL 时，证书要由公开的 CA 签发", "With SSL, the certificate must come from a public CA"), size=14, color=INK3,
          align="center")


def compose_page(p):
    """A new agent session: the lamp listens. Hold the AI key anywhere, or hold
    the lamp on this page, and talk; the lamp turns violet and rises with your
    voice. Then the words become the card the session will be, and holding the
    bar sends it, as answering and stopping are holds too."""
    g = "st_compose"
    group(p, g, 0, 0, W, H, hidden=True)
    button(g, "cp_cancel", 372, 40, 84, 36, text=CANCEL, size=16, bg=CLEAR, fg=INK2,
           border=None, pressed_dim=24, callback="cp_cancel")

    # Stage: ready, listening, transcribing. The lamp sits where the home lamp does.
    s = "cp_stage"
    group(g, s, 0, 0, W, H)
    label(s, 24, 48, 340, 22, "", font=MONO, size=14, color=INK2, name="cp_where", bind=True,
          ellipsis=True)
    add_orb(s, "cp_orb", 90, 74, 300, color=VOICE, value=20, particles=0, strength=40)
    icon = under(s, {"type": "image", "name": "cp_mic_icon", "x": 222, "y": 168, "w": 36,
                     "h": 36, "image": "icons/mic.svg", "tint": INK1})
    visibility(icon)
    label(s, 110, 196, 260, 56, "", size=40, align="center", name="cp_big", bind=True)
    label(s, 110, 212, 260, 30, "", size=19, color=VOICE, align="center", name="cp_word",
          bind=True, ellipsis=True)
    # The lamp is the button: hold it to talk.
    button(s, "cp_orb_btn", 110, 94, 260, 260, bg=CLEAR, border=None, radius=130, pressed_dim=0,
           events=calls(press="mic_press", release="mic_release", cancel="mic_release"))
    label(s, 28, 386, 424, 62, "", size=21, align="center", name="cp_live", bind=True)
    shimmer = under(s, {"type": "effect", "name": "cp_shimmer", "x": 150, "y": 410, "w": 180,
                        "h": 6, "effect": "shimmer", "fg_color": VOICE_DIM, "bg_color": "#141416",
                        "period_ms": 1400, "hidden": True})
    visibility(shimmer)
    label(s, 28, 446, 424, 22, "", size=14, color=INK3, align="center", name="cp_hint",
          bind=True, ellipsis=True)
    keys = button(s, "cp_keys", 16, 437, 120, 40, text=tr("键盘", "Keyboard"), size=15,
                  bg=CLEAR, fg=INK2, border=None, pressed_dim=24, callback="cp_keys", hidden=True)
    visibility(keys)

    # Review: the card this session will be, and the hold-to-send bar.
    r = "cp_review"
    group(g, r, 0, 0, W, H, hidden=True)
    rect(r, 28, 53, 8, 8, VOICE, radius=4)
    label(r, 44, 45, 300, 24, tr("新会话", "New session"), size=17, color=VOICE)
    label(r, 28, 72, 420, 22, "", size=15, color=INK2, name="cp_meta", bind=True, ellipsis=True)
    # The card's own title geometry (card_page), so this is the card to be.
    label(r, 28, 96, 420, 180, "", size=26, name="cp_title", bind=True)
    rect(r, 28, 286, 424, 1, LINE)
    rect(r, 28, 309, 8, 8, BG, radius=4, border=VOICE, border_w=2)
    label(r, 44, 300, 300, 24, "", size=15, color=INK2, name="cp_again", bind=True,
          ellipsis=True)
    button(r, "cp_keys2", 336, 294, 120, 36, text=tr("键盘", "Keyboard"), size=15, bg=CLEAR,
           fg=INK2, border=None, pressed_dim=24, callback="cp_keys")
    under(r, {"type": "progress", "name": "cp_fill", "x": 24, "y": 372, "w": 432, "h": 64,
              "value": 0, "bg_color": TILE, "fg_color": VOICE_DEEP, "radius": 32})
    label(r, 24, 390, 432, 28, "", size=19, align="center", name="cp_send_t", bind=True)
    button(r, "cp_send", 24, 372, 432, 64, bg=CLEAR, border=None, radius=32, pressed_dim=0,
           events=calls(press="send_press", long="send_long", release="send_release",
                        cancel="send_release"))


def input_page(p):
    """Typed entry: one field and the on-screen keyboard. The logic attaches the
    keyboard to `in_text` for each step (direct connection: host, port,
    password; a daemon password Paseo asked for; a Wi-Fi password) and reads
    the text back on OK."""
    g = "st_input"
    group(p, g, 0, 0, W, H, hidden=True)
    page_header(g, "in", "", CANCEL, "in_cancel", bind_title=True)
    label(g, 24, 80, 432, 20, "", size=13, color=INK3, name="in_caption", bind=True, ellipsis=True)
    field = group(g, "in_field", 24, 104, 432, 52, vis=False, bg=SURF)
    field.update({"radius": 14, "border_color": EDGE, "border_width": 1})
    label("in_field", 16, 12, 400, 28, "", font=MONO, size=18, name="in_text", bind=True,
          input=True)
    label(g, 24, 162, 432, 22, "", size=14, color=FAIL, name="in_error", bind=True, ellipsis=True)
    under(g, {"type": "keyboard", "name": "kb", "x": 0, "y": 196, "w": W, "h": H - 196,
              "font": NOTO, "font_size": 20, "function_font_size": 15, "bg_color": "#0B0B0A",
              "fg_color": INK1, "key_color": "#1F1E1C", "function_color": "#2A2926",
              "function_text_color": INK2, "delete_color": "#2A2926", "delete_text_color": INK2,
              "space_color": "#1F1E1C", "space_text_color": INK2, "ok_color": INK1,
              "ok_text_color": BG, "key_radius": 10,
              # The keyboard's labels have no bind: the same in both languages.
              "shift_label": "Aa", "delete_label": "←", "ok_label": "OK",
              "symbols_label": "?123", "letters_label": "ABC", "space_label": ""})


def wifi_page(p):
    """Networks from the last scan, strongest first. A saved or open network
    connects on tap; a protected one asks for its password."""
    g = "st_wifi"
    group(p, g, 0, 0, W, H, hidden=True)
    page_header(g, "wf", tr("选择网络", "Choose network"), CANCEL, "wf_cancel")
    button(g, "wf_rescan", 268, 40, 100, 36, text=tr("重新扫描", "Rescan"), size=16, bg=CLEAR, fg=INK2,
           border=None, pressed_dim=24, callback="wf_rescan")
    label(g, 24, 230, 432, 24, "", size=16, color=INK3, align="center", name="wf_note", bind=True)
    for i in range(WIFI_ROWS):
        row = f"wf_row{i}"
        btn = button(g, row, 16, 88 + i * 60, 448, 54, bg=TILE, border=None, radius=16,
                     pressed_dim=32, callback=row, hidden=True)
        visibility(btn)
        label(row, 18, 6, 360, 24, "", size=18, name=f"wf_name{i}", bind=True, ellipsis=True)
        label(row, 18, 31, 360, 18, "", size=13, color=INK3, name=f"wf_meta{i}", bind=True,
              ellipsis=True)
        for k, h in enumerate((6, 10, 14)):
            rect(row, 404 + k * 8, 34 - h, 5, h, DIM, radius=1, name=f"wf_sig{i}_{k}",
                 color_bind=True)


def net_details_page(p):
    g = "st_netinfo"
    group(p, g, 0, 0, W, H, hidden=True)
    page_header(g, "ni", "Wi-Fi", BACK, "ni_close")
    label(g, 24, 96, 432, 36, "", size=28, name="ni_ssid", bind=True, ellipsis=True)
    label(g, 24, 138, 432, 22, "", size=15, color=INK2, name="ni_signal", bind=True, ellipsis=True)
    label(g, 24, 162, 432, 22, "", font=MONO, size=15, color=INK2, name="ni_ip", bind=True,
          ellipsis=True)
    rect(g, 24, 202, 432, 1, LINE)
    label(g, 24, 216, 432, 20, tr("已保存的网络", "Saved networks"), size=13, color=INK3)
    for i in range(SAVED_ROWS):
        label(g, 24, 242 + i * 28, 432, 24, "", size=17, name=f"ni_saved{i}", bind=True,
              ellipsis=True)
    button(g, "ni_pick", 40, 380, 196, 52, text=tr("选择网络", "Choose network"), size=18, callback="wifi_pick")
    button(g, "ni_scan", 244, 380, 196, 52, text=tr("扫码加入", "Scan to join"), size=18, bg=INK1, fg=BG,
           border=None, callback="scan_open")


def link_details_page(p):
    g = "st_link"
    group(p, g, 0, 0, W, H, hidden=True)
    page_header(g, "lk", "Paseo", BACK, "lk_close")
    label(g, 24, 96, 432, 36, "", size=28, name="lk_host", bind=True, ellipsis=True)
    label(g, 24, 138, 432, 22, "", font=MONO, size=15, color=INK2, name="lk_url", bind=True,
          ellipsis=True)
    label(g, 24, 162, 432, 22, "", size=15, color=INK2, name="lk_state", bind=True, ellipsis=True)
    label(g, 24, 186, 432, 22, "", size=15, color=INK3, name="lk_device", bind=True,
          ellipsis=True)
    button(g, "lk_rescan", 40, 380, 196, 52, text=tr("重新连接", "Reconnect"), size=18, callback="connect_open")
    button(g, "lk_forget", 244, 380, 196, 52, size=18, callback="lk_forget")
    label("lk_forget", 0, 12, 196, 26, "", size=18, color=FAIL, align="center",
          name="lk_forget_t", bind=True)


def hold_overlay():
    layer = {"type": "layer", "name": "ov_hold", "x": 0, "y": BAR_H, "w": W, "h": H - BAR_H,
             "bg_color": "#000000E8", "hidden": True}
    under(None, layer)
    visibility(layer)
    under("ov_hold", {"type": "arc", "name": "hold_ring", "x": 140, "y": 84, "w": 200, "h": 200,
                      "min": 0, "max": 100, "value": 0, "start_angle": 0, "sweep": 360,
                      "thickness": 4, "bg_color": "#1F1F1D", "fg_color": FAIL})
    label("ov_hold", 140, 150, 200, 40, tr("停止", "Stop"), size=28, align="center")
    label("ov_hold", 140, 192, 200, 24, tr("继续按住", "Keep holding"), size=15, color=INK2, align="center")
    label("ov_hold", 40, 314, 400, 26, tr("松手就取消。停止只打断这一轮，", "Let go to cancel. Stopping ends only this turn;"), size=16, color=INK2,
          align="center")
    label("ov_hold", 40, 340, 400, 26, tr("agent 和它的上下文都还在。", "the agent and its context stay."), size=16, color=INK2,
          align="center")


def banner():
    g = group(None, "banner", 24, 42, 432, 32, hidden=True, bg="#1A1A18")
    g["radius"] = 16
    label("banner", 16, 5, 400, 22, "", size=14, color=INK1, name="banner_text", bind=True,
          ellipsis=True)
    button("banner", "banner_btn", 0, 0, 432, 32, bg=CLEAR, border=None, radius=16,
           pressed_dim=24, callback="banner_tap")


def control_centre():
    under(None, {"type": "drawer", "name": "cc", "x": 0, "y": 0, "w": W, "h": 380, "edge": "top",
                 "open": False, "bg_color": "#0B0B0AF8"})
    p = "cc"
    label(p, 24, 40, 300, 22, "", size=15, color=INK2, name="cc_date", bind=True)

    # Wi-Fi tile
    tile = group(p, "cc_wifi", 24, 70, 210, 92, vis=False, bg=TILE)
    tile["radius"] = 22
    for k, (x, h) in enumerate([(20, 8), (27, 13), (34, 18)]):
        rect("cc_wifi", x, 34 - h, 5, h, INK1, radius=2, name=f"cc_wb{k}", color_bind=True)
    label("cc_wifi", 20, 42, 180, 24, "Wi-Fi", font=G_M, size=17)
    label("cc_wifi", 20, 66, 180, 20, "", size=14, color=INK2, name="cc_wifi_sub", bind=True,
          ellipsis=True)
    button("cc_wifi", "cc_wifi_btn", 0, 0, 210, 92, bg=CLEAR, border=None, radius=22,
           pressed_dim=32, callback="cc_wifi")

    # Paseo tile
    tile = group(p, "cc_paseo", 246, 70, 210, 92, vis=False, bg=TILE)
    tile["radius"] = 22
    rect("cc_paseo", 20, 16, 18, 18, TILE, radius=9, border=INK1, border_w=2, name="cc_link_ring")
    dot = rect("cc_paseo", 25, 21, 8, 8, INK1, radius=4, name="cc_link_dot", hidden=True)
    visibility(dot)
    label("cc_paseo", 20, 42, 180, 24, "", font=G_M, size=17, name="cc_host", bind=True,
          ellipsis=True)
    label("cc_paseo", 20, 66, 180, 20, "", size=14, color=INK2, name="cc_paseo_sub", bind=True,
          ellipsis=True)
    button("cc_paseo", "cc_paseo_btn", 0, 0, 210, 92, bg=CLEAR, border=None, radius=22,
           pressed_dim=32, callback="cc_paseo")

    # toggles: an off and an on container each, switched by visibility
    for x, key, name in [(24, "dnd", tr("勿扰", "Quiet")), (174, "sound", tr("声音", "Sound")),
                         (324, "haptic", tr("振动", "Haptics"))]:
        t = f"cc_{key}"
        for state, bg, fg, sub, word in (("off", TILE, INK1, INK2, tr("关", "Off")),
                                         ("on", INK1, BG, "#3A3936", tr("开", "On"))):
            v = f"{t}_{state}"
            g = group(p, v, x, 174, 132, 76, hidden=state == "on", bg=bg)
            g["radius"] = 20
            label(v, 18, 16, 100, 24, name, size=17, color=fg)
            label(v, 18, 42, 100, 20, word, size=14, color=sub)
        button(p, f"{t}_btn", x, 174, 132, 76, bg=CLEAR, border=None, radius=20, pressed_dim=32,
               callback=t)
    under(p, {"type": "image", "x": 30, "y": 275, "w": 18, "h": 18, "image": "icons/sun.svg",
              "tint": INK2})
    under(p, {"type": "slider", "name": "cc_bright", "x": 58, "y": 266, "w": 318, "h": 36,
              "min": 5, "max": 100, "value": 72, "track_size": 36, "radius": 18, "bg_color": TILE,
              "fg_color": "#CFCBC2", "knob_color": INK1, "callback": "cc_bright"})
    auto = button(p, "cc_auto", 388, 266, 68, 36, text=tr("自动", "Auto"), size=14, bg=TILE, border="#3A3936",
                  radius=18, callback="cc_auto")
    auto["bind"] = "cc_auto_bg"
    auto["bind_target"] = "color"
    auto["pressed_color"] = "#2A2926"
    del auto["pressed_dim"]
    label(p, 24, 322, 330, 22, "", size=14, color=INK3, name="cc_batt", bind=True, ellipsis=True)
    button(p, "cc_about", 356, 316, 100, 32, text=tr("关于 ›", "About ›"), size=14, bg=CLEAR, fg=INK2, border=None,
           pressed_dim=24, callback="cc_about")
    rect(p, 220, 360, 40, 4, "#3A3936", radius=2)


def status_bar():
    group(None, "sb", 0, 0, W, BAR_H, vis=False, drift=True)
    label("sb", 24, 9, 90, 22, "00:00", font=G_M, size=16, name="sb_time", bind=True,
          charset=DIGITS + ":-")
    # Hidden while the fuel gauge has not answered.
    group("sb", "sb_batt_box", 0, 0, W, BAR_H, hidden=True)
    rect("sb_batt_box", 426, 13, 24, 12, BG, radius=3, border=INK2)
    # A rect with both a dynamic width and a color bind fails to load in GSP 1.9.0
    # ("Invalid GSB render effects"), and a Progress's bind slot carries its value,
    # so the low-battery color is a second Progress switched by visibility.
    for name, fg in (("sb_batt", INK1), ("sb_batt_low", FAIL)):
        batt = under("sb_batt_box", {"type": "progress", "name": name, "x": 428, "y": 15, "w": 20,
                                     "h": 8, "value": 87, "bg_color": BG, "fg_color": fg,
                                     "radius": 2, "hidden": name == "sb_batt_low"})
        visibility(batt)
    rect("sb_batt_box", 451, 17, 2, 4, INK2, radius=1)
    bolt = under("sb_batt_box", {"type": "image", "name": "sb_bolt", "x": 432, "y": 13, "w": 12,
                                 "h": 12, "image": "icons/bolt.svg", "tint": BG, "hidden": True})
    visibility(bolt)
    label("sb_batt_box", 380, 10, 42, 20, "", font=MONO, size=13, color=INK2, align="right",
          name="sb_batt_pct", bind=True, charset=DIGITS + "%")
    for k, (x, h) in enumerate([(362, 5), (367, 8), (372, 11)]):
        rect("sb", x, 26 - h, 3, h, DIM, radius=1, name=f"sb_wifi{k}", color_bind=True)
    rect("sb", 360, 12, 17, 2, FAIL, radius=1, name="sb_wifi_off", visible_bind=True, hidden=True)
    rect("sb", 339, 14, 11, 11, BG, radius=6, border=INK2, border_w=2, name="sb_link",
         visible_bind=True)
    rect("sb", 339, 14, 11, 11, BG, radius=6, border=FAIL, border_w=2, name="sb_link_off",
         visible_bind=True, hidden=True)
    rect("sb", 342, 17, 5, 5, INK2, radius=3, name="sb_link_dot", visible_bind=True, hidden=True)
    moon = under("sb", {"type": "image", "name": "sb_dnd", "x": 316, "y": 13, "w": 13, "h": 13,
                        "image": "icons/moon.svg", "tint": INK2, "hidden": True})
    visibility(moon)


def build() -> dict:
    objects.clear()
    texts.clear()
    under(None, {"type": "page_flow", "name": "screens", "x": 0, "y": 0, "w": W, "h": H,
                 "page_count": 3, "selected": 1, "axis": "horizontal", "bar_height": 0,
                 "pages": ["usage", "home", "deck"]})
    for k, page in enumerate(("usage", "home", "deck")):
        under("screens", {"type": "layer", "name": page, "x": W * k, "y": 0, "w": W, "h": H,
                          "bg_color": BG})
    usage_page("usage")
    home_page("home")
    deck_page("deck")
    status_pages()
    hold_overlay()
    banner()
    control_centre()
    status_bar()
    return {
        "screen": "app", "w": W, "h": H, "screen_bg": BG, "font": NOTO, "default_font_size": 18,
        "swipe": False,
        "objects": objects,
    }


# ---------------------------------------------------------------- Rust tables

RUST_OUT = ROOT / "rust/logic/src/scene.rs"
VARIANT_KEYS = list(STATE_COLORS)


def k(name: str) -> str:
    return f"gsp::GSP_OBJ_KEY_{name.upper()}"


def b(name: str) -> str:
    return f"gsp::GSP_BIND_{name.upper()} as Bind"


def variants(name: str) -> str:
    items = ", ".join(f"Variant {{ vis: {b(name + '_' + v + '_vis')}, text: {b(name + '_' + v + '_t')} }}"
                      for v in VARIANT_KEYS)
    return f"[{items}]"


def rust_tables() -> str:
    cards = []
    for i in range(CARDS):
        c = f"c{i}"
        opts = []
        for j in range(OPTIONS):
            o = f"{c}_o{j}"
            opts.append(f"""            Opt {{
                vis: {b(o + '_vis')},
                key_color: {b(o + '_k_color')},
                text: {b(o + '_t')},
                fill: {b(o + '_fill_vis')},
                fill_text: {b(o + '_ft')},
                ring: {b(o + '_ring_vis')},
            }},""")
        cards.append(f"""    Card {{
        dot: {b(c + '_dot')},
        word: {variants(c + '_word')},
        meta: {b(c + '_meta')},
        title: {b(c + '_title')},
        ask: {b(c + '_q_vis')},
        ask_text: {b(c + '_q_text')},
        options: [
{chr(10).join(opts)}
        ],
        ask_hint: {b(c + '_q_hint')},
        run: {b(c + '_run_vis')},
        timer: {b(c + '_run_timer')},
        now: {b(c + '_run_now')},
        alive: {b(c + '_run_alive_vis')},
        recent: [{b(c + '_run_a0')}, {b(c + '_run_a1')}, {b(c + '_run_a2')}],
        bar: {k(c + '_run_bar')},
        todo: {b(c + '_run_todo')},
        run_hint: {b(c + '_run_hint')},
        done: {b(c + '_done_vis')},
        err: {b(c + '_done_err')},
        body: {b(c + '_done_body')},
        body_key: {k(c + '_done_body')},
        cost: {b(c + '_done_cost')},
    }},""")
    usage = []
    for i in range(USAGE_CELLS):
        u = f"u{i}"
        usage.append(f"""    UsageCell {{
        vis: {b(u + '_vis')},
        arcs: [{k(u + '_arc_work')}, {k(u + '_arc_call')}, {k(u + '_arc_fail')}],
        arc_vis: [{b(u + '_arc_work_vis')}, {b(u + '_arc_call_vis')}, {b(u + '_arc_fail_vis')}],
        pct: {b(u + '_pct')},
        name: {b(u + '_name')},
        reset: {b(u + '_reset')},
    }},""")
    home_dots = ", ".join(f"Dot {{ key: {k(f'home_dot{i}')}, color: {b(f'home_dot{i}')} }}"
                          for i in range(FLEET_DOTS))
    rail_dots = ", ".join(f"Dot {{ key: {k(f'rail{i}')}, color: {b(f'rail{i}')} }}"
                          for i in range(FLEET_DOTS))
    wifi_rows = "\n".join(
        f"    WifiRow {{ vis: {b(f'wf_row{i}_vis')}, name: {b(f'wf_name{i}')}, "
        f"meta: {b(f'wf_meta{i}')}, bars: [{', '.join(b(f'wf_sig{i}_{j}') for j in range(3))}] }},"
        for i in range(WIFI_ROWS))
    saved_rows = ", ".join(b(f"ni_saved{i}") for i in range(SAVED_ROWS))
    return f"""// @generated by tools/ui/gen_scene.py; do not edit.
//! Typed tables of the scene's keys and binds, grouped the way the views use
//! them. Regenerated with the scene, so a renamed object is a compile error.

use gsp_sys as gsp;

use crate::ui::{{Bind, Key}};

/// Text that changes color: one container per color, each with its own label.
#[derive(Clone, Copy)]
pub struct Variant {{
    pub vis: Bind,
    pub text: Bind,
}}

/// Order of [`Variant`] arrays: call, work, done, fail, dim.
pub const VARIANTS: usize = {len(VARIANT_KEYS)};

#[derive(Clone, Copy)]
pub struct Opt {{
    pub vis: Bind,
    pub key_color: Bind,
    pub text: Bind,
    pub fill: Bind,
    pub fill_text: Bind,
    pub ring: Bind,
}}

#[derive(Clone, Copy)]
pub struct Card {{
    pub dot: Bind,
    pub word: [Variant; VARIANTS],
    pub meta: Bind,
    pub title: Bind,
    pub ask: Bind,
    pub ask_text: Bind,
    pub options: [Opt; {OPTIONS}],
    pub ask_hint: Bind,
    pub run: Bind,
    pub timer: Bind,
    pub now: Bind,
    pub alive: Bind,
    pub recent: [Bind; 3],
    pub bar: Key,
    pub todo: Bind,
    pub run_hint: Bind,
    pub done: Bind,
    pub err: Bind,
    pub body: Bind,
    pub body_key: Key,
    pub cost: Bind,
}}

#[derive(Clone, Copy)]
pub struct UsageCell {{
    pub vis: Bind,
    /// One Arc per color: work, call, fail.
    pub arcs: [Key; 3],
    pub arc_vis: [Bind; 3],
    pub pct: Bind,
    pub name: Bind,
    pub reset: Bind,
}}

#[derive(Clone, Copy)]
pub struct Dot {{
    pub key: Key,
    pub color: Bind,
}}

/// One network row on the "choose a network" page; `bars` are color binds.
#[derive(Clone, Copy)]
pub struct WifiRow {{
    pub vis: Bind,
    pub name: Bind,
    pub meta: Bind,
    pub bars: [Bind; 3],
}}

pub const CARDS: [Card; {CARDS}] = [
{chr(10).join(cards)}
];

pub const USAGE: [UsageCell; {USAGE_CELLS}] = [
{chr(10).join(usage)}
];

pub const HOME_DOTS: [Dot; {FLEET_DOTS}] = [{home_dots}];
pub const RAIL_DOTS: [Dot; {FLEET_DOTS}] = [{rail_dots}];
pub const HOME_WORD: [Variant; VARIANTS] = {variants('home_word')};
pub const WIFI_ROWS: [WifiRow; {WIFI_ROWS}] = [
{wifi_rows}
];
pub const SAVED_ROWS: [Bind; {SAVED_ROWS}] = [{saved_rows}];

/// Every translated label (`tr(...)` in the generator): its bind, then the
/// text in Chinese and in English. The scene starts in Chinese.
pub const TEXTS: [(Bind, &str, &str); {len(texts)}] = [
{chr(10).join(f"    ({b(name)}, {json.dumps(zh, ensure_ascii=False)}, {json.dumps(en, ensure_ascii=False)})," for name, zh, en in texts)}
];
"""


if __name__ == "__main__":
    scene = build()
    OUT.write_text(json.dumps(scene, ensure_ascii=False, indent=1) + "\n")
    RUST_OUT.write_text(rust_tables())
    # Same layout as `cargo fmt`, so regenerating leaves no diff.
    subprocess.run(["rustfmt", "--edition", "2024", str(RUST_OUT)], cwd=RUST_OUT.parent, check=True)
    print(f"{OUT.relative_to(ROOT)}: {len(scene['objects'])} objects; {RUST_OUT.relative_to(ROOT)}")
