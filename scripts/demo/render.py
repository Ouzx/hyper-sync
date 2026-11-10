"""Turn record.py's recordings into the README media (docs/media): a mock-up of the monitor on a wall
with the strip glowing behind it, the TUI on screen. LED colors come from the real effect engine;
the screen sync tile is simulated. Needs Pillow, ffmpeg and the JetBrains Mono / Noto fonts.

    python3 scripts/demo/render.py all
"""
import json, math, os, re, shutil, subprocess, sys
from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HERE = os.path.join(REPO, "target", "demo")
OUT = os.path.join(HERE, "out")
MEDIA = os.path.join(REPO, "docs", "media")
FONT = "/usr/share/fonts/jetbrains-mono-fonts/JetBrainsMono-Regular.otf"
FONT_B = "/usr/share/fonts/jetbrains-mono-fonts/JetBrainsMono-Bold.otf"
FALLBACK = "/usr/share/fonts/google-noto/NotoSansSymbols2-Regular.ttf"
SIZE = 16
font, font_b = ImageFont.truetype(FONT, SIZE), ImageFont.truetype(FONT_B, SIZE)
fallback = ImageFont.truetype(FALLBACK, SIZE)
CW, CH = 10, 21
DEF_FG, DEF_BG = (204, 208, 216), (17, 19, 24)
BASE16 = [(30, 32, 38), (224, 108, 117), (152, 195, 121), (229, 192, 123), (97, 175, 239), (198, 120, 221),
          (86, 182, 194), (190, 195, 205), (92, 99, 112), (240, 128, 136), (170, 215, 140), (240, 205, 140),
          (120, 190, 245), (215, 140, 235), (110, 200, 210), (240, 242, 245)]

# ---------------------------------------------------------------- terminal

def xterm256(n):
    if n < 16:
        return BASE16[n]
    if n < 232:
        n -= 16
        lv = [0, 95, 135, 175, 215, 255]
        return (lv[n // 36], lv[n // 6 % 6], lv[n % 6])
    v = 8 + 10 * (n - 232)
    return (v, v, v)


SGR = re.compile(r"\x1b\[([0-9;:]*)m")


def parse(ansi):
    """ANSI text from `tmux capture-pane -e` -> rows of (char, fg, bg, bold)."""
    # The recording ran with a scratch config dir; show the usual path.
    scratch = "(%s|%s)" % (re.escape(HERE), re.escape(HERE.replace(os.path.expanduser("~"), "~", 1)))
    ansi = re.sub(scratch + r"[^\x1b\s│]*", lambda m: "~/.config/hyper-sync/config.toml".ljust(len(m.group(0))), ansi)
    rows = []
    fg = bg = None
    bold = dim = rev = False
    for line in ansi.split("\n"):
        row, pos = [], 0
        for m in list(SGR.finditer(line)) + [None]:
            end = m.start() if m else len(line)
            for ch in line[pos:end]:
                f, b = fg or DEF_FG, bg or DEF_BG
                if dim:
                    f = tuple(int(c * 0.6 + d * 0.4) for c, d in zip(f, b))
                if rev:
                    f, b = b, f
                row.append((ch, f, b, bold))
            if not m:
                break
            pos = m.end()
            ps = [p for p in m.group(1).replace(":", ";").split(";")] or ["0"]
            i = 0
            while i < len(ps):
                p = int(ps[i]) if ps[i] else 0
                if p == 0:
                    fg = bg = None; bold = dim = rev = False
                elif p == 1: bold = True
                elif p == 2: dim = True
                elif p == 22: bold = dim = False
                elif p == 7: rev = True
                elif p == 27: rev = False
                elif p in (38, 48):
                    if ps[i + 1] == "5":
                        c = xterm256(int(ps[i + 2])); i += 2
                    else:
                        rgb = ps[i + 2:i + 5]
                        if rgb[0] == "" and len(ps) > i + 5:  # 38:2::r:g:b
                            rgb = ps[i + 3:i + 6]; i += 1
                        c = tuple(int(x) for x in rgb); i += 4
                    if p == 38: fg = c
                    else: bg = c
                elif p == 39: fg = None
                elif p == 49: bg = None
                elif 30 <= p <= 37: fg = BASE16[p - 30]
                elif 90 <= p <= 97: fg = BASE16[p - 90 + 8]
                elif 40 <= p <= 47: bg = BASE16[p - 40]
                elif 100 <= p <= 107: bg = BASE16[p - 100 + 8]
                i += 1
        rows.append(row)
    while rows and not rows[-1]:
        rows.pop()
    return rows


# Box drawing: (left, right, up, down) arms.
BOX = {"─": "lr", "━": "lr", "│": "ud", "┃": "ud", "┌": "rd", "╭": "rd", "┐": "ld", "╮": "ld", "└": "ru",
       "╰": "ru", "┘": "lu", "╯": "lu", "├": "rud", "┤": "lud", "┬": "lrd", "┴": "lru", "┼": "lrud"}
EIGHTHS = "▁▂▃▄▅▆▇█"
def _ink(f, ch):
    im = Image.new("L", (CW * 2, CH))
    ImageDraw.Draw(im).text((0, 0), ch, font=f, fill=255)
    return im.tobytes()


_notdef = _ink(font, "\U0010FFFD")
_has = {}


def glyph_font(ch, bold):
    if ch not in _has:
        _has[ch] = ch.isascii() or _ink(font, ch) != _notdef
    return (font_b if bold else font) if _has[ch] else fallback


def draw_terminal(rows, cols, nrows):
    img = Image.new("RGB", (cols * CW, nrows * CH), DEF_BG)
    d = ImageDraw.Draw(img)
    for y, row in enumerate(rows[:nrows]):
        for x, (ch, fg, bg, bold) in enumerate(row[:cols]):
            x0, y0 = x * CW, y * CH
            if bg != DEF_BG:
                d.rectangle([x0, y0, x0 + CW - 1, y0 + CH - 1], fill=bg)
            if ch == " ":
                continue
            if ch in BOX:
                cx, cy = x0 + CW // 2, y0 + CH // 2
                arms = BOX[ch]
                if "l" in arms: d.line([x0, cy, cx, cy], fill=fg)
                if "r" in arms: d.line([cx, cy, x0 + CW - 1, cy], fill=fg)
                if "u" in arms: d.line([cx, y0, cx, cy], fill=fg)
                if "d" in arms: d.line([cx, cy, cx, y0 + CH - 1], fill=fg)
            elif ch in EIGHTHS:
                h = round(CH * (EIGHTHS.index(ch) + 1) / 8)
                d.rectangle([x0, y0 + CH - h, x0 + CW - 1, y0 + CH - 1], fill=fg)
            elif ch == "▀":
                d.rectangle([x0, y0, x0 + CW - 1, y0 + CH // 2 - 1], fill=fg)
            elif ch == "░":
                d.rectangle([x0, y0, x0 + CW - 1, y0 + CH - 1], fill=tuple((f + 3 * b) // 4 for f, b in zip(fg, bg)))
            else:
                d.text((x0, y0 + 2), ch, font=glyph_font(ch, bold), fill=fg)
    return img


def window(term, title):
    """A terminal window with a title bar."""
    pad, bar = 12, 34
    w, h = term.width + 2 * pad, term.height + 2 * pad + bar
    win = Image.new("RGB", (w, h), DEF_BG)
    d = ImageDraw.Draw(win)
    d.rectangle([0, 0, w, bar], fill=(32, 34, 41))
    for i, c in enumerate([(237, 106, 94), (245, 191, 79), (98, 197, 84)]):
        d.ellipse([14 + i * 22, bar // 2 - 6, 26 + i * 22, bar // 2 + 6], fill=c)
    tw = font.getlength(title)
    d.text(((w - tw) / 2, bar // 2 - 10), title, font=font, fill=(150, 155, 165))
    win.paste(term, (pad, bar + pad))
    mask = Image.new("L", win.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, w - 1, h - 1], 10, fill=255)
    return win, mask


# ---------------------------------------------------------------- scene

W, H = 1920, 1080
SX, SY, SW, SH = 288, 104, 1344, 756  # screen
BEZEL = 12


def wall():
    img = Image.linear_gradient("L").resize((W, H)).point(lambda v: v)
    top, bot = (24, 25, 30), (12, 13, 16)
    base = Image.new("RGB", (W, H))
    px = [tuple(int(t + (b - t) * y / H) for t, b in zip(top, bot)) for y in range(H)]
    d = ImageDraw.Draw(base)
    for y, c in enumerate(px):
        d.line([0, y, W, y], fill=c)
    d.rectangle([0, 1012, W, H], fill=(20, 21, 25))
    d.line([0, 1012, W, 1012], fill=(38, 40, 46))
    return base


WALL = wall()


def led_positions(n=65):
    """Where each LED sits on the back of the monitor (LED 0 bottom-right, counter-clockwise)."""
    right, top = n * 17 // 65, n * 31 // 65
    left = n - right - top
    inset = 26
    x0, x1, y0, y1 = SX + inset, SX + SW - inset, SY + inset, SY + SH - inset
    pts = []
    for k in range(right):
        pts.append((x1, y1 - (y1 - y0) * (k + 0.5) / right))
    for k in range(top):
        pts.append((x1 - (x1 - x0) * (k + 0.5) / top, y0))
    for k in range(left):
        pts.append((x0, y0 + (y1 - y0) * (k + 0.5) / left))
    return pts


POS = led_positions()


def encode(v):
    """LEDs are linear light; the image is sRGB."""
    return (v / 255) ** (1 / 2.2) * 255


def glow(colors, gain=1.0):
    q = 4
    layer = Image.new("RGB", (W // q, H // q))
    d = ImageDraw.Draw(layer)
    for (x, y), c in zip(POS, colors):
        c = tuple(min(255, int(encode(v) * gain)) for v in c)
        r = 9
        d.ellipse([x / q - r, y / q - r, x / q + r, y / q + r], fill=c)
    wide = layer.filter(ImageFilter.GaussianBlur(26)).resize((W, H), Image.BILINEAR)
    tight = layer.filter(ImageFilter.GaussianBlur(7)).resize((W, H), Image.BILINEAR)
    wide = wide.point(lambda v: min(255, int(v * 2.4)))
    tight = tight.point(lambda v: min(255, int(v * 1.1)))
    return ImageChops.add(wide, tight)


def wallpaper():
    img = Image.new("RGB", (SW, SH))
    d = ImageDraw.Draw(img)
    for y in range(SH):
        t = y / SH
        d.line([0, y, SW, y], fill=(int(14 + 10 * t), int(16 + 6 * t), int(28 + 14 * t)))
    return img


WALLPAPER = wallpaper()


def scene(colors, screen, gain=1.0):
    img = ImageChops.add(WALL, glow(colors, gain))
    d = ImageDraw.Draw(img)
    # stand
    d.rounded_rectangle([W // 2 - 48, SY + SH, W // 2 + 48, 1000], 6, fill=(36, 38, 44))
    d.rounded_rectangle([W // 2 - 220, 994, W // 2 + 220, 1014], 8, fill=(44, 46, 52))
    # body + bezel
    d.rounded_rectangle([SX - BEZEL, SY - BEZEL, SX + SW + BEZEL, SY + SH + BEZEL + 8], 10, fill=(22, 23, 27))
    d.rounded_rectangle([SX - BEZEL, SY - BEZEL, SX + SW + BEZEL, SY + SH + BEZEL + 8], 10, outline=(48, 50, 56))
    img.paste(screen, (SX, SY))
    return img


def screen_with(win_mask):
    win, mask = win_mask
    s = WALLPAPER.copy()
    s.paste(win, ((SW - win.width) // 2, (SH - win.height) // 2), mask)
    return s


def sunset():
    """A made-up scene for the (simulated) screen sync still."""
    img = Image.new("RGB", (SW, SH))
    d = ImageDraw.Draw(img)
    stops = [(0.0, (28, 30, 92)), (0.35, (126, 52, 140)), (0.6, (236, 104, 82)), (0.72, (252, 176, 76))]
    for y in range(SH):
        t = y / SH
        for (a, ca), (b, cb) in zip(stops, stops[1:] + [(1.0, stops[-1][1])]):
            if a <= t <= b:
                k = (t - a) / max(b - a, 1e-6)
                d.line([0, y, SW, y], fill=tuple(int(x + (z - x) * k) for x, z in zip(ca, cb)))
                break
    d.ellipse([SW * 0.62, SH * 0.48, SW * 0.62 + 170, SH * 0.48 + 170], fill=(255, 214, 120))
    hills = [(0, SH)] + [(x, SH * 0.66 + 40 * math.sin(x / 140) + 30 * math.sin(x / 57)) for x in range(0, SW + 1, 16)] + [(SW, SH)]
    d.polygon(hills, fill=(18, 40, 58))
    sea = [(0, SH)] + [(x, SH * 0.82 + 14 * math.sin(x / 90)) for x in range(0, SW + 1, 16)] + [(SW, SH)]
    d.polygon(sea, fill=(10, 24, 44))
    return img.filter(ImageFilter.GaussianBlur(1))


def edge_colors(img):
    """Average the screen edge next to each LED, as screen sync does (here already sRGB, so no gamma)."""
    out = []
    band = 0.07
    for x, y in POS:
        u, v = (x - SX) / SW, (y - SY) / SH
        if abs(x - (SX + SW - 26)) < 1:
            box = (SW * (1 - band), v * SH - 22, SW, v * SH + 22)
        elif abs(x - (SX + 26)) < 1:
            box = (0, v * SH - 22, SW * band, v * SH + 22)
        else:
            box = (u * SW - 22, 0, u * SW + 22, SH * band)
        px = img.crop(tuple(int(b) for b in box)).resize((1, 1), Image.BOX).getpixel((0, 0))
        out.append(tuple(int(((c / 255) ** 2.2) * 255) for c in px))  # to linear, like the daemon
    return out


def triples(flat):
    return [tuple(flat[i:i + 3]) for i in range(0, len(flat), 3)]


# ---------------------------------------------------------------- outputs

def render_demo(limit=None):
    frames = [json.loads(l) for l in open(os.path.join(HERE, "tui.jsonl"))]
    fdir = os.path.join(OUT, "frames")
    os.makedirs(fdir, exist_ok=True)
    last_ansi, last_screen = None, None
    for i, fr in enumerate(frames[:limit]):
        if fr["ansi"] != last_ansi:
            last_ansi = fr["ansi"]
            last_screen = screen_with(window(draw_terminal(parse(fr["ansi"]), 100, 32), "hyper-sync tui"))
        scene(triples(fr["leds"]), last_screen, gain=1.25).save(os.path.join(fdir, "%04d.png" % i))
        if i % 50 == 0:
            print("frame", i, flush=True)
    return len(frames[:limit])


def render_tui_png(g):
    win, mask = window(draw_terminal(parse(g["_tui"]), 100, 32), "hyper-sync tui")
    out = Image.new("RGBA", win.size)
    out.paste(win, (0, 0), mask)
    out.save(os.path.join(OUT, "tui.png"))


def render_gallery(g, picks):
    tw, th, cap = 640, 360, 46
    cols = 3
    rows = (len(picks) + cols - 1) // cols
    grid = Image.new("RGB", (cols * tw, rows * (th + cap)), (12, 13, 16))
    d = ImageDraw.Draw(grid)
    label_font = ImageFont.truetype(FONT, 22)
    for k, (key, label) in enumerate(picks):
        if key == "screen":
            sc = sunset()
            tile = scene(edge_colors(sc), sc, gain=1.0)
        else:
            tile = scene(triples(g[key]), WALLPAPER, gain=1.0)
        tile = tile.crop((96, 0, W - 96, H - 30)).resize((tw, th), Image.LANCZOS)
        x, y = (k % cols) * tw, (k // cols) * (th + cap)
        grid.paste(tile, (x, y))
        lw = label_font.getlength(label)
        d.text((x + (tw - lw) / 2, y + th + 9), label, font=label_font, fill=(190, 194, 204))
    grid.save(os.path.join(OUT, "effects.png"))


GALLERY = [("screen", "Screen sync (simulated)"), ("aurora", "Aurora"), ("fire", "Fire"),
           ("flow", "Rainbow flow"), ("mood_blobs", "Mood blobs"), ("knight_rider", "Knight rider"),
           ("lava", "Lava"), ("police", "Police lights"), ("plasma", "Plasma")]


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    g = json.load(open(os.path.join(HERE, "gallery.json")))
    what = sys.argv[1]
    if what == "still":
        frames = [json.loads(l) for l in open(os.path.join(HERE, "tui.jsonl"))]
        fr = frames[int(sys.argv[2])]
        scene(triples(fr["leds"]), screen_with(window(draw_terminal(parse(fr["ansi"]), 100, 32), "hyper-sync tui")), gain=1.25).save(os.path.join(OUT, "still.png"))
    elif what == "demo":
        print(render_demo())
    elif what == "tui":
        render_tui_png(g)
    elif what == "all":
        render_tui_png(g)
        render_gallery(g, GALLERY)
        shutil.rmtree(os.path.join(OUT, "frames"), ignore_errors=True)
        render_demo()
        ff = ["ffmpeg", "-v", "error", "-y", "-framerate", "20", "-i", os.path.join(OUT, "frames", "%04d.png")]
        subprocess.run(ff + ["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "20", "-preset", "slow",
                             "-movflags", "+faststart", os.path.join(OUT, "demo.mp4")], check=True)
        subprocess.run(ff + ["-vf", "fps=12,crop=1440:1000:240:40,scale=1080:-1:flags=lanczos,split[a][b];"
                             "[a]palettegen=max_colors=192:stats_mode=diff[p];"
                             "[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle",
                             os.path.join(OUT, "demo.gif")], check=True)
        os.makedirs(MEDIA, exist_ok=True)
        for name in ["demo.gif", "demo.mp4", "tui.png", "effects.png"]:
            shutil.copy(os.path.join(OUT, name), MEDIA)
    elif what == "gallery":
        render_gallery(g, GALLERY)
