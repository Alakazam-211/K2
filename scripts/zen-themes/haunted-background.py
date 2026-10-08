#!/usr/bin/env python3
"""Draw the built-in `haunted` Zen theme's background (the Diary's scene).

A candlelit desk in a dark, misty study: wood grain, a candle's glow, cobwebs
in the upper corners, mist and dust. Drawn procedurally with Pillow (no
external assets, no network), seeded, so the output is the same every run:

    python3 scripts/zen-themes/haunted-background.py \
        crates/k2-core/src/zen/themes/haunted-background.webp

The middle of the frame stays calm: the Diary's book sits there. The theme
shows it full-bleed (`fit = "cover"`), so it reads at any window size.
Rosson 2026-10-08: the scene lives in the theme, never in the widget.
"""

import math
import random
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter

W, H = 1920, 1200
DESK_Y = int(H * 0.60)  # the desk's back edge
CANDLE = (int(W * 0.13), int(H * 0.47))  # the flame
rng = random.Random(20261008)


def radial(size, inner, outer, power=1.0):
    """An 'L' mask: `inner` at the centre fading to `outer` at the edge."""
    w, h = size
    # Pillow's gradient is 0 at the centre and 181 at the middle of each
    # edge (255 only in the corners): 181 is "the edge" here.
    g = Image.radial_gradient("L").resize((w, h), Image.BICUBIC)
    lut = []
    for i in range(256):
        t = min(1.0, i / 181.0)
        t = t ** power
        lut.append(int(round(inner + (outer - inner) * t)))
    return g.point(lut)


def paste_glow(base, color, center, size, strength, power=1.6):
    """Add a soft coloured glow (screen-like add) at `center`."""
    cx, cy = center
    w, h = size
    mask = radial((w, h), strength, 0, power)
    layer = Image.new("RGB", (w, h), color)
    glow = Image.new("RGB", base.size, (0, 0, 0))
    glow_mask = Image.new("L", base.size, 0)
    glow.paste(layer, (cx - w // 2, cy - h // 2))
    glow_mask.paste(mask, (cx - w // 2, cy - h // 2))
    lit = ImageChops.add(base, Image.composite(glow, Image.new("RGB", base.size, 0), glow_mask))
    return lit


def wall(img):
    """The study's back wall: near-black, a little purple, faint panels."""
    d = ImageDraw.Draw(img)
    for y in range(DESK_Y):
        t = y / DESK_Y
        r = int(14 + 10 * t)
        g = int(10 + 7 * t)
        b = int(16 + 6 * t)
        d.line([(0, y), (W, y)], fill=(r, g, b))
    # Wainscot panels, barely there.
    panel = Image.new("L", (W, H), 0)
    pd = ImageDraw.Draw(panel)
    x = -60
    while x < W:
        pw = rng.randint(300, 380)
        pd.rectangle([x + 18, int(H * 0.08), x + pw - 18, DESK_Y - 30], outline=70, width=3)
        x += pw
    panel = panel.filter(ImageFilter.GaussianBlur(2.5))
    img.paste((44, 32, 38), mask=panel)
    return img


def shelf(img):
    """A bookshelf in the dark on the right wall: dusty spines, two boards."""
    d = ImageDraw.Draw(img)
    x0, x1 = int(W * 0.72), int(W * 0.97)
    for top, bottom in ((int(H * 0.16), int(H * 0.33)), (int(H * 0.36), int(H * 0.53))):
        x = x0
        while x < x1 - 20:
            bw = rng.randint(14, 34)
            bh = rng.randint(int((bottom - top) * 0.62), bottom - top - 4)
            lean = rng.choice([0, 0, 0, rng.randint(-10, 10)])
            hue = rng.choice([(46, 22, 20), (28, 30, 40), (38, 30, 18), (24, 26, 22), (44, 34, 30)])
            d.polygon([(x, bottom), (x + bw, bottom), (x + bw + lean, bottom - bh), (x + lean, bottom - bh)], fill=hue)
            d.line([(x + 3 + lean * 0.5, bottom - bh * 0.7), (x + bw - 3 + lean * 0.5, bottom - bh * 0.7)], fill=(70, 56, 40), width=1)
            x += bw + rng.randint(0, 3)
        d.rectangle([x0 - 16, bottom, x1 + 16, bottom + 12], fill=(40, 26, 18))
    return img


def desk(img):
    """A dark walnut desk top with wavy grain, lit from the candle side."""
    top = Image.new("RGB", (W, H - DESK_Y), (34, 21, 14))
    d = ImageDraw.Draw(top)
    hh = H - DESK_Y
    for i in range(240):
        y0 = rng.uniform(-10, hh + 10)
        amp = rng.uniform(1.5, 7.0)
        freq = rng.uniform(0.002, 0.007)
        phase = rng.uniform(0, math.tau)
        shade = rng.randint(-22, 24)
        col = (max(0, 48 + shade), max(0, 29 + shade // 2), max(0, 18 + shade // 3))
        width = rng.choice([1, 1, 2, 2, 3])
        pts = []
        for x in range(0, W + 16, 16):
            # The grain runs along the desk and bunches near knots.
            y = y0 + amp * math.sin(x * freq + phase) + (x / W) * rng.uniform(-0.4, 0.4)
            pts.append((x, y))
        d.line(pts, fill=col, width=width)
    for _ in range(5):  # knots
        kx, ky = rng.randint(200, W - 200), rng.randint(20, hh - 20)
        for r in range(26, 2, -3):
            d.ellipse([kx - r * 2.2, ky - r * 0.7, kx + r * 2.2, ky + r * 0.7], outline=(24, 14, 9), width=1)
    top = top.filter(ImageFilter.GaussianBlur(0.8))
    # The front of the desk falls into shadow; the back edge catches light.
    shade = Image.linear_gradient("L").resize((W, hh))  # 0 top, 255 bottom
    dark = Image.new("RGB", (W, hh), (6, 4, 3))
    top = Image.composite(dark, top, shade.point(lambda v: int(v * 0.72)))
    img.paste(top, (0, DESK_Y))
    d = ImageDraw.Draw(img)
    d.line([(0, DESK_Y), (W, DESK_Y)], fill=(74, 48, 30), width=3)
    d.line([(0, DESK_Y + 3), (W, DESK_Y + 3)], fill=(20, 12, 8), width=4)
    return img


def candle(img):
    """A stub of candle in a brass holder, and its glow on wall and desk."""
    cx, cy = CANDLE
    img = paste_glow(img, (230, 110, 40), (cx, cy + 40), (1700, 1400), 140, 1.25)
    img = paste_glow(img, (255, 150, 70), (cx, cy), (560, 560), 150, 1.8)
    d = ImageDraw.Draw(img)
    # The holder's dish on the desk.
    dish_y = DESK_Y + 90
    d.ellipse([cx - 120, dish_y - 26, cx + 120, dish_y + 26], fill=(88, 62, 30))
    d.ellipse([cx - 100, dish_y - 22, cx + 100, dish_y + 12], fill=(122, 88, 44))
    # The wax, dripped.
    d.rectangle([cx - 34, cy + 40, cx + 34, dish_y - 6], fill=(196, 170, 128))
    d.ellipse([cx - 34, cy + 28, cx + 34, cy + 52], fill=(226, 200, 150))
    for k in range(4):
        dx = rng.randint(-30, 26)
        d.rounded_rectangle([cx + dx, cy + 44, cx + dx + 8, cy + 44 + rng.randint(30, 110)], radius=4, fill=(212, 186, 140))
    # Shade the wax's far side.
    sh = Image.new("L", img.size, 0)
    ImageDraw.Draw(sh).rectangle([cx + 6, cy + 40, cx + 34, dish_y - 6], fill=90)
    img.paste((70, 50, 30), mask=sh.filter(ImageFilter.GaussianBlur(6)))
    # Wick and flame.
    d = ImageDraw.Draw(img)
    d.line([(cx, cy + 30), (cx + 2, cy + 18)], fill=(30, 20, 14), width=3)
    flame = Image.new("L", img.size, 0)
    fd = ImageDraw.Draw(flame)
    fd.ellipse([cx - 13, cy - 34, cx + 13, cy + 22], fill=255)
    fd.polygon([(cx - 11, cy - 10), (cx + 11, cy - 10), (cx + 3, cy - 62)], fill=255)
    flame = flame.filter(ImageFilter.GaussianBlur(4))
    img.paste((255, 214, 140), mask=flame)
    core = Image.new("L", img.size, 0)
    ImageDraw.Draw(core).ellipse([cx - 5, cy - 8, cx + 5, cy + 16], fill=230)
    img.paste((255, 250, 228), mask=core.filter(ImageFilter.GaussianBlur(3)))
    return img


def cobweb(img, corner, size, flip):
    """A spider web anchored in a top corner: radials and sagging rings."""
    layer = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(layer)
    ox, oy = corner
    spokes = 9
    angles = [math.radians(5 + i * (80 / (spokes - 1))) for i in range(spokes)]
    def pt(a, r):
        x = ox + (r * math.cos(a) if not flip else -r * math.cos(a))
        return (x, oy + r * math.sin(a))
    for a in angles:
        d.line([pt(a, 0), pt(a, size * rng.uniform(0.85, 1.0))], fill=210, width=2)
    r = size * 0.12
    while r < size * 0.95:
        for i in range(spokes - 1):
            a0, a1 = angles[i], angles[i + 1]
            p0, p1 = pt(a0, r), pt(a1, r)
            mid = pt((a0 + a1) / 2, r * 0.9)  # sags toward the corner
            d.line([p0, mid, p1], fill=170, width=1)
        r *= rng.uniform(1.18, 1.3)
    # A loose strand drifting down.
    sx = ox + (size * 0.55 if not flip else -size * 0.55)
    d.line([(sx, oy + size * 0.5), (sx + (12 if not flip else -12), oy + size * 1.25)], fill=120, width=1)
    layer = layer.filter(ImageFilter.GaussianBlur(0.7))
    img.paste((214, 208, 200), mask=layer.point(lambda v: int(v * 0.7)))
    return img


def mist(img):
    """Low mist over the desk and a haze in the room."""
    fog = Image.new("L", (W // 4, H // 4), 0)
    d = ImageDraw.Draw(fog)
    for _ in range(70):
        x = rng.uniform(-50, W // 4 + 50)
        y = rng.uniform(H // 4 * 0.45, H // 4 * 1.05)
        rx, ry = rng.uniform(30, 110), rng.uniform(6, 18)
        d.ellipse([x - rx, y - ry, x + rx, y + ry], fill=rng.randint(30, 66))
    for _ in range(14):
        x = rng.uniform(0, W // 4)
        y = rng.uniform(0, H // 4 * 0.5)
        r = rng.uniform(40, 90)
        d.ellipse([x - r, y - r * 0.5, x + r, y + r * 0.5], fill=rng.randint(6, 14))
    fog = fog.filter(ImageFilter.GaussianBlur(14)).resize((W, H), Image.BICUBIC)
    tint = Image.new("RGB", (W, H), (150, 160, 190))
    return Image.composite(tint, img, fog)


def dust(img):
    """Motes hanging in the candle light."""
    layer = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(layer)
    cx, cy = CANDLE
    for _ in range(260):
        a = rng.uniform(0, math.tau)
        r = abs(rng.gauss(0, 330))
        x, y = cx + r * math.cos(a) * 1.4, cy - 60 + r * math.sin(a)
        s = rng.choice([1, 1, 1.5, 2])
        d.ellipse([x - s, y - s, x + s, y + s], fill=rng.randint(60, 170))
    layer = layer.filter(ImageFilter.GaussianBlur(0.7))
    img.paste((255, 220, 170), mask=layer)
    return img


def vignette(img):
    """Darken the edges, and keep the middle (the book's place) calm."""
    mask = radial((W, H), 0, 205, 2.1)
    return Image.composite(Image.new("RGB", (W, H), (4, 3, 4)), img, mask)


def main(out):
    img = Image.new("RGB", (W, H), (12, 9, 12))
    img = wall(img)
    img = shelf(img)
    img = desk(img)
    img = candle(img)
    img = cobweb(img, (W, 0), 430, True)
    img = cobweb(img, (0, 0), 300, False)
    img = mist(img)
    img = dust(img)
    img = vignette(img)
    img.save(out, "WEBP", quality=72, method=6)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: haunted-background.py <out.webp>")
    main(sys.argv[1])
