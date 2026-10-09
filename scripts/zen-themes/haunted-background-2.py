#!/usr/bin/env python3
"""Draw the second `haunted` Zen background (Rosson 2026-10-08: "not
haunted enough", then "sharper, higher resolution").

A candlelit study at night, at 3840x2400 so it stays sharp on a Retina
screen: cracked, water-stained plaster over wood-panelled wainscot; a
moonlit arched window with mullions, leaded panes, a bare tree outside and
a hand pressed on the glass, throwing a cold shaft of light down onto the
desk; shelves of jars, books and a skull; cobwebs in the corners with a
spider on its thread; days scratched into the wall; a face in the desk's
grain; mist, dust, film grain and a dark that closes in at the edges.

Procedural, seeded and Pillow-only (no outside art, no network, no numpy),
so the output is the same every run:

    python3 scripts/zen-themes/haunted-background-2.py \
        crates/k2-core/src/zen/themes/haunted-background-2.webp

The middle stays calm and dark: the Diary's book sits there, so the detail
lives in the left and right quarters. The places the Diary widget animates
over the image line up with it: the candle flame at 13% / 47%, the beam
under the window top right, the face in the grain low on the right.
"""

import math
import random
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageMath, ImageOps

W, H = 3840, 2400
CORNICE = 56  # the dark band along the ceiling
RAIL_Y = int(H * 0.385)  # the chair rail: plaster above, panels below
DESK_Y = int(H * 0.60)  # the desk's back edge
CANDLE = (int(W * 0.13), int(H * 0.47))  # the flame
# The window, top right: left, arch top, right, bottom of the glass.
WIN = (int(W * 0.775), int(H * 0.035), int(W * 0.925), int(H * 0.335))
# Where the moonbeam lands on the desk (the patch's centre).
LANDING = (int(W * 0.745), int(H * 0.69))
# The face in the grain (matches the widget's `.face`, right 9% / bottom 13%).
FACE = (int(W * 0.8536), int(H * 0.80))
rng = random.Random(20261031)

BLACK = (0, 0, 0)


# ── small tools ────────────────────────────────────────────────────────


def solid(color, size=(W, H)):
    return Image.new("RGB", size, color)


def noise(w, h):
    """Uniform noise from the seeded generator (Pillow's own is unseeded)."""
    return Image.frombytes("L", (w, h), rng.randbytes(w * h))


def fractal(size, cells, octaves=5, persistence=0.55):
    """Value noise: upsampled random grids, summed, stretched to 0..255."""
    acc, total, amp = None, 0.0, 1.0
    cw, ch = cells
    for _ in range(octaves):
        n = noise(max(2, int(cw)), max(2, int(ch))).resize(size, Image.BICUBIC)
        if acc is None:
            acc, total = n, amp
        else:
            acc = Image.blend(acc, n, amp / (total + amp))
            total += amp
        amp *= persistence
        cw, ch = cw * 2, ch * 2
    return ImageOps.autocontrast(acc, cutoff=0.5)


def radial(size, inner, outer, power=1.0):
    """An 'L' mask: `inner` at the centre fading to `outer` at the edge."""
    w, h = size
    # Pillow's gradient is 0 at the centre and 181 at the middle of each
    # edge (255 only in the corners): 181 is "the edge" here.
    g = Image.radial_gradient("L").resize((w, h), Image.BICUBIC)
    lut = [int(round(inner + (outer - inner) * min(1.0, i / 181.0) ** power)) for i in range(256)]
    return g.point(lut)


def scale(mask, k):
    return mask.point(lambda v: int(v * k))


def tint(img, color, mask):
    """Paint `color` through `mask` (0 keeps the image, 255 is the colour)."""
    img.paste(color, mask=mask)
    return img


def light(img, color, mask):
    """Add light: `color` scaled by `mask`, added to the image."""
    return ImageChops.add(img, Image.composite(solid(color, img.size), solid(BLACK, img.size), mask))


def glow(img, color, center, size, strength, power=1.6):
    """Add a soft coloured glow at `center`."""
    cx, cy = center
    w, h = size
    m = Image.new("L", img.size, 0)
    m.paste(radial((w, h), strength, 0, power), (cx - w // 2, cy - h // 2))
    return light(img, color, m)


def coord(w, h, axis):
    """An 'F' image holding each pixel's x (axis 0) or y (axis 1)."""
    if axis == 0:
        line = Image.new("F", (w, 1))
        line.putdata([float(i) for i in range(w)])
    else:
        line = Image.new("F", (1, h))
        line.putdata([float(i) for i in range(h)])
    return line.resize((w, h), Image.NEAREST)


def ss_layer(size, k=2):
    """A supersampled 'L' layer and its draw: thin lines come out clean."""
    lay = Image.new("L", (size[0] * k, size[1] * k), 0)
    return lay, ImageDraw.Draw(lay)


def ss_done(lay, size):
    return lay.resize(size, Image.LANCZOS)


# ── wood ───────────────────────────────────────────────────────────────


def wood(w, h, dark, lite, period, warp_amp, knots=(), fiber=0.42, cells=(3, 2)):
    """Wood with the grain running along x: growth rings warped by noise and
    bent around knots, and fine fibres. Returns RGB (w, h)."""
    warp = fractal((max(8, w // 8), max(8, h // 8)), cells, 4).resize((w, h), Image.BICUBIC).convert("F")
    bump = Image.new("F", (w, h), 0.0)
    for kx, ky, kr, ks in knots:  # a knot: rings circle it
        bw, bh = int(kr * 5.0), int(kr * 2.0)
        g = radial((bw, bh), 0, 255, 1.0).point([int(255 * max(0.0, 1 - i / 255) ** 2) for i in range(256)])
        crop = g.convert("F").point(lambda v, ks=ks: v * ks / 255.0)
        bump.paste(crop, (int(kx - bw / 2), int(ky - bh / 2)))
    ys = coord(w, h, 1)
    xs = coord(w, h, 0)
    slope = rng.uniform(-0.012, 0.012)
    f = ImageMath.lambda_eval(
        lambda e: ((e["y"] + e["x"] * slope + 1000.0) / period + e["n"] * (warp_amp / 255.0) + e["b"]) % 1.0 * 255.0,
        y=ys, x=xs, n=warp, b=bump,
    ).convert("L")
    # Earlywood fades light to dark across the ring; latewood is a thin dark line.
    lut = []
    for i in range(256):
        r = i / 255.0
        v = 0.75 - 0.35 * r
        if r > 0.86:
            v = 0.12 + (r - 0.86) * 2.0
        lut.append(int(max(0, min(255, v * 255))))
    rings = f.point(lut).filter(ImageFilter.GaussianBlur(0.9))
    fibres = noise(max(2, w // 110), max(2, h // 2)).resize((w, h), Image.BICUBIC)
    mask = Image.blend(rings, fibres, fiber)
    pores = noise(w // 2, h // 2).resize((w, h), Image.NEAREST).point(lambda v: 255 if v > 246 else 0)
    out = Image.composite(solid(lite, (w, h)), solid(dark, (w, h)), mask)
    return tint(out, dark, scale(pores, 0.5))


# ── the wall ───────────────────────────────────────────────────────────


def wall():
    """Plaster above the rail, raised wood panels below, a cornice on top."""
    col = Image.new("RGB", (1, H))
    col.putdata([(int(13 + 11 * t), int(9 + 8 * t), int(14 + 6 * t)) for t in (y / DESK_Y if y < DESK_Y else 1.0 for y in range(H))])
    base = col.resize((W, H), Image.NEAREST)
    # Plaster: mottled, pitted, uneven.
    mott = Image.blend(fractal((W // 2, H // 2), (14, 9), 6, 0.6), noise(W // 2, H // 2), 0.22).resize((W, H), Image.BICUBIC)
    img = Image.composite(ImageChops.add(base, solid((7, 5, 5))), ImageChops.subtract(base, solid((7, 5, 6))), mott)
    pits = noise(W, H).point(lambda v: 255 if v > 251 else 0).filter(ImageFilter.GaussianBlur(0.7))
    img = tint(img, (5, 4, 5), scale(pits, 0.7))
    d = ImageDraw.Draw(img)
    # The cornice, with a lip that catches a little light, and its shadow.
    d.rectangle([0, 0, W, CORNICE], fill=(9, 7, 9))
    d.line([(0, CORNICE), (W, CORNICE)], fill=(40, 32, 30), width=4)
    sh = Image.new("L", (W, H), 0)
    ImageDraw.Draw(sh).rectangle([0, CORNICE + 3, W, CORNICE + 40], fill=150)
    img = tint(img, (4, 3, 4), sh.filter(ImageFilter.GaussianBlur(14)))

    # Wainscot: vertical-grain panels between the rail and the desk.
    top = RAIL_Y + 34
    ph = DESK_Y - top
    tex = wood(ph, W, (14, 8, 7), (44, 28, 19), 26.0, 5.0, fiber=0.5).transpose(Image.ROTATE_90)
    img.paste(tex, (0, top))
    d = ImageDraw.Draw(img)
    hi = Image.new("L", (W, H), 0)
    lo = Image.new("L", (W, H), 0)
    hd, ld = ImageDraw.Draw(hi), ImageDraw.Draw(lo)
    x = -110
    while x < W:
        pw = rng.randint(420, 470)
        l, r = x + 58, x + pw - 58
        t, b = top + 54, DESK_Y - 46
        # The raised field: a bevel lit from above left, dark below right.
        hd.line([(l, b), (l, t), (r, t)], fill=120, width=6)
        ld.line([(l, b), (r, b), (r, t)], fill=200, width=8)
        hd.line([(l + 30, b - 30), (r - 30, b - 30), (r - 30, t + 30)], fill=60, width=3)
        ld.line([(l + 30, b - 30), (l + 30, t + 30), (r - 30, t + 30)], fill=150, width=4)
        ld.line([(x, top), (x, DESK_Y)], fill=120, width=3)  # the joint between stiles
        x += pw
    img = tint(img, (70, 50, 36), hi.filter(ImageFilter.GaussianBlur(1.6)))
    img = tint(img, (3, 2, 2), lo.filter(ImageFilter.GaussianBlur(2.2)))
    # The chair rail: a moulding with a lit top and a shadow under it.
    d = ImageDraw.Draw(img)
    d.rectangle([0, RAIL_Y, W, top], fill=(26, 16, 11))
    d.line([(0, RAIL_Y), (W, RAIL_Y)], fill=(78, 54, 36), width=4)
    d.line([(0, RAIL_Y + 14), (W, RAIL_Y + 14)], fill=(46, 30, 20), width=3)
    d.line([(0, top), (W, top)], fill=(6, 4, 3), width=5)
    sh = Image.new("L", (W, H), 0)
    ImageDraw.Draw(sh).rectangle([0, top, W, top + 30], fill=170)
    return tint(img, (4, 3, 3), sh.filter(ImageFilter.GaussianBlur(10)))


def stain(img, cx, cy, rx, ry, drips):
    """A water stain: a damp patch with tide lines, weeping downward."""
    bw, bh = int(rx * 2), int(ry * 2 + drips * 1.2)
    ox, oy = int(cx - rx), int(cy - ry)
    q = 2
    sw, sh = bw // q, bh // q
    field = fractal((sw, sh), (5, 5 * sh / max(1, sw) + 1), 5, 0.6)
    fall = Image.new("L", (sw, sh), 0)
    fall.paste(radial((sw, int(ry * 2 / q)), 255, 0, 1.4), (0, 0))
    field = ImageChops.multiply(Image.blend(field, fall, 0.55), fall.point(lambda v: min(255, v * 3)))
    body = Image.new("L", (sw, sh), 0)
    rims = Image.new("L", (sw, sh), 0)
    for k, t in enumerate((70, 92, 112, 128)):
        m = field.point(lambda v, t=t: 255 if v > t else 0)
        body = ImageChops.lighter(body, scale(m, 0.18 + 0.08 * k))
        edge = ImageChops.subtract(m, m.filter(ImageFilter.MinFilter(3)))
        rims = ImageChops.lighter(rims, scale(edge, 0.85 - 0.12 * k))
    # Runs: damp streaks from the bottom of the stain.
    run = Image.new("L", (sw, sh), 0)
    rd = ImageDraw.Draw(run)
    for _ in range(int(rx / 22)):
        x = rng.uniform(sw * 0.2, sw * 0.8)
        y0 = ry / q * rng.uniform(0.9, 1.3)
        ln = rng.uniform(0.3, 1.0) * drips / q
        rd.line([(x, y0), (x + rng.uniform(-2, 2), y0 + ln)], fill=rng.randint(60, 140), width=rng.choice([1, 2, 2, 3]))
    run = run.filter(ImageFilter.GaussianBlur(1.5))
    body = ImageChops.lighter(body, scale(run, 0.8))
    body = body.resize((bw, bh), Image.BICUBIC).filter(ImageFilter.GaussianBlur(2))
    rims = rims.resize((bw, bh), Image.BICUBIC).filter(ImageFilter.GaussianBlur(1.1))
    mb = Image.new("L", img.size, 0)
    mr = Image.new("L", img.size, 0)
    mb.paste(body, (ox, oy))
    mr.paste(rims, (ox, oy))
    img = tint(img, (30, 21, 12), scale(mb, 0.95))
    return tint(img, (64, 48, 30), scale(mr.filter(ImageFilter.GaussianBlur(1.6)), 0.5))


def crack(dk, hl, x, y, ang, length, width, depth):
    """One crack: a jagged walk that thins and sometimes splits."""
    while length > 0:
        seg = rng.uniform(8, 28)
        ang += rng.gauss(0, 0.38)
        nx, ny = x + math.cos(ang) * seg, y + math.sin(ang) * seg
        dk.line([(x, y), (nx, ny)], fill=255, width=max(1, int(round(width))))
        hl.line([(x + 1.5, y + 2.5), (nx + 1.5, ny + 2.5)], fill=255, width=1)
        if depth < 3 and rng.random() < 0.07:
            crack(dk, hl, nx, ny, ang + rng.choice([-1, 1]) * rng.uniform(0.5, 1.1), length * rng.uniform(0.3, 0.55), width * 0.65, depth + 1)
        x, y = nx, ny
        length -= seg
        width = max(1.0, width * 0.988)


def cracks(img):
    dk = Image.new("L", img.size, 0)
    hl = Image.new("L", img.size, 0)
    dd, hd = ImageDraw.Draw(dk), ImageDraw.Draw(hl)
    for x, y, ang, ln, wd in (
        (int(W * 0.205), CORNICE + 2, 1.95, 760, 5.0),   # from the cornice, top left
        (int(W * 0.03), RAIL_Y - 4, -1.35, 420, 3.5),    # up from the rail
        (int(W * 0.768), int(H * 0.12), 2.9, 520, 4.0),  # out of the window's arch
        (int(W * 0.938), int(H * 0.30), 0.9, 300, 3.0),  # from the sill's end
        (int(W * 0.985), CORNICE + 2, 1.7, 540, 4.0),    # down the right corner
    ):
        crack(dd, hd, x, y, ang, ln, wd, 0)
    img = tint(img, (7, 5, 6), scale(dk.filter(ImageFilter.GaussianBlur(9)), 0.35))  # grime along it
    img = tint(img, (4, 3, 3), scale(dk.filter(ImageFilter.GaussianBlur(0.6)), 0.92))
    hl = ImageChops.subtract(hl, dk)
    return tint(img, (70, 60, 54), scale(hl.filter(ImageFilter.GaussianBlur(0.6)), 0.45))


def tallies(img):
    """Days counted by someone who never left: scratched into the wall."""
    layer = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(layer)
    x0, y0 = int(W * 0.045), int(H * 0.17)
    for row in range(4):
        x = x0 + rng.randint(-16, 16)
        y = y0 + row * 128
        for group in range(rng.randint(3, 5)):
            marks = 5 if (row, group) != (3, 0) else rng.randint(1, 3)
            for m in range(min(marks, 4)):
                tilt = rng.uniform(-10, 10)
                d.line([(x + m * 22, y), (x + m * 22 + tilt, y + 88)], fill=rng.randint(120, 190), width=5)
            if marks == 5:
                d.line([(x - 12, y + 68), (x + 84, y + 16)], fill=170, width=5)
            x += 128
    gouge = layer.filter(ImageFilter.GaussianBlur(0.8))
    img = tint(img, (5, 4, 4), scale(ImageChops.offset(gouge, -2, -2), 0.7))  # the cut's shadow
    return tint(img, (176, 152, 128), scale(gouge, 0.85))


def handprint(img, cx, cy, size_px, angle, alpha, color, smear=7):
    """A hand, palm and splayed fingers, dragged a little down."""
    size = int(size_px)
    hand = Image.new("L", (size, size * 2), 0)
    d = ImageDraw.Draw(hand)
    px, py = size // 2, int(size * 1.05)
    pr = size * 0.2
    d.ellipse([px - pr, py - pr * 1.1, px + pr, py + pr * 1.1], fill=255)
    for i, (a, ln) in enumerate([(-62, 0.30), (-28, 0.42), (-6, 0.46), (16, 0.42), (38, 0.33)]):
        r = math.radians(a - 90)
        bx, by = px + math.cos(r) * pr * 0.8, py + math.sin(r) * pr * 0.8
        ex, ey = bx + math.cos(r) * size * ln, by + math.sin(r) * size * ln
        d.line([(bx, by), (ex, ey)], fill=255, width=int(size * (0.075 if i else 0.09)))
        d.ellipse([ex - size * 0.04, ey - size * 0.04, ex + size * 0.04, ey + size * 0.04], fill=255)
    # Ridges: the print is patchy, not a stamp.
    hand = ImageChops.multiply(hand, noise(size // 3, size * 2 // 3).resize((size, size * 2), Image.BICUBIC).point(lambda v: min(255, 80 + v)))
    base = hand.copy()
    for k in range(1, smear + 1):
        moved = base.transform(base.size, Image.AFFINE, (1, 0, 0, 0, 1, -k * size * 0.03))
        hand = ImageChops.lighter(hand, scale(moved, 0.82 ** k))
    hand = hand.rotate(angle, resample=Image.BICUBIC, expand=True).filter(ImageFilter.GaussianBlur(2.6))
    mask = Image.new("L", img.size, 0)
    mask.paste(scale(hand, alpha), (int(cx - hand.width / 2), int(cy - hand.height / 2)))
    return tint(img, color, mask)


def drips(img):
    """Something dark weeping from the cornice."""
    layer = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(layer)
    for x in (W * 0.035, W * 0.232, W * 0.738, W * 0.952):
        x = int(x + rng.randint(-40, 40))
        length = rng.randint(int(H * 0.06), int(H * 0.24))
        w = rng.uniform(4, 7)
        pts = [(x + math.sin(y / 74) * 3, y) for y in range(CORNICE, length, 8)]
        d.line(pts, fill=230, width=int(w))
        d.ellipse([x - w * 1.4, length - w, x + w * 1.4, length + w * 2.2], fill=240)
        for _ in range(rng.randint(0, 2)):  # thinner runs off the main one
            y0 = rng.randint(CORNICE, length)
            d.line([(x, y0), (x + rng.randint(-6, 6), y0 + rng.randint(40, 160))], fill=170, width=2)
    return tint(img, (30, 4, 6), scale(layer.filter(ImageFilter.GaussianBlur(1.6)), 0.75))


# ── the window ─────────────────────────────────────────────────────────


def arch(d, box, fill, grow=0):
    l, t, r, b = box
    l, t, r, b = l - grow, t - grow, r + grow, b + grow
    rad = (r - l) / 2
    d.pieslice([l, t, r, t + 2 * rad], 180, 360, fill=fill)
    d.rectangle([l, t + rad, r, b], fill=fill)


def window_bars(d, box, fill, k=1.0):
    """The frame's mullion and transoms, and the arch's ribs."""
    l, t, r, b = box
    rad = (r - l) / 2
    cx, cy = (l + r) / 2, t + rad
    d.line([(cx, t), (cx, b)], fill=fill, width=int(24 * k))
    for f in (0.0, 0.5):
        y = cy + (b - cy) * f
        d.line([(l, y), (r, y)], fill=fill, width=int(20 * k))
    for a in (-45, 45):
        ra = math.radians(a - 90)
        d.line([(cx, cy), (cx + math.cos(ra) * rad, cy + math.sin(ra) * rad)], fill=fill, width=int(14 * k))


def tree(d, x, y, ang, ln, wd, depth):
    """A bare tree's branch, forking as it reaches."""
    nx, ny = x + math.cos(ang) * ln, y + math.sin(ang) * ln
    steps = 6
    px, py = x, y
    for i in range(1, steps + 1):
        t = i / steps
        qx = x + (nx - x) * t + rng.gauss(0, ln * 0.03)
        qy = y + (ny - y) * t + rng.gauss(0, ln * 0.03)
        d.line([(px, py), (qx, qy)], fill=255, width=max(1, int(wd * (1 - 0.35 * t))))
        px, py = qx, qy
    if depth < 6:
        for _ in range(rng.choice([2, 2, 3])):
            tree(d, px, py, ang + rng.uniform(-0.75, 0.75), ln * rng.uniform(0.55, 0.78), max(1.0, wd * 0.62), depth + 1)


def glass_mask(box, size=(W, H), k=1.0):
    l, t, r, b = (int(v * k) for v in box)
    m = Image.new("L", size, 0)
    arch(ImageDraw.Draw(m), (l, t, r, b), 255)
    return m


def window(img):
    l, t, r, b = WIN
    rad = (r - l) // 2
    d = ImageDraw.Draw(img)
    # The deep reveal in a thick wall, and the sill.
    reveal = Image.new("L", img.size, 0)
    arch(ImageDraw.Draw(reveal), WIN, 255, grow=54)
    img = tint(img, (6, 5, 8), reveal.filter(ImageFilter.GaussianBlur(3)))
    # The night outside: sky, moon, clouds, a dead tree.
    gm = glass_mask(WIN)
    sky = Image.new("RGB", (W, H))
    col = Image.new("RGB", (1, H))
    col.putdata([(int(16 + 14 * s), int(20 + 18 * s), int(36 + 26 * s)) for s in (max(0.0, 1 - abs(y - (t + rad * 0.9)) / 700) for y in range(H))])
    sky = col.resize((W, H), Image.NEAREST)
    mx, my = int(l + (r - l) * 0.33), int(t + rad * 0.72)
    sky = glow(sky, (90, 104, 140), (mx, my), (900, 900), 120, 1.4)
    md = Image.new("L", img.size, 0)
    ImageDraw.Draw(md).ellipse([mx - 74, my - 74, mx + 74, my + 74], fill=255)
    moon = Image.composite(solid((206, 212, 224)), solid((150, 160, 182)), fractal((W // 8, H // 8), (40, 25), 3).resize((W, H), Image.BICUBIC))
    sky.paste(moon, mask=md.filter(ImageFilter.GaussianBlur(1.5)))
    clouds = fractal((W // 4, H // 4), (8, 30), 5, 0.6).resize((W, H), Image.BICUBIC).point(lambda v: max(0, (v - 120) * 2))
    sky = tint(sky, (34, 38, 54), scale(clouds, 0.8))
    branches = Image.new("L", img.size, 0)
    bd = ImageDraw.Draw(branches)
    tree(bd, r + 40, b + 30, math.radians(-128), 260, 26, 0)
    tree(bd, l - 30, t + rad * 1.8, math.radians(-38), 200, 14, 1)
    sky = tint(sky, (5, 5, 8), branches.filter(ImageFilter.GaussianBlur(1.2)))
    # Old glass: a little wavy and dirty, leaded in diamonds.
    dirt = fractal((W // 4, H // 4), (30, 20), 4).resize((W, H), Image.BICUBIC)
    sky = tint(sky, (20, 22, 28), scale(dirt, 0.35))
    lead = Image.new("L", img.size, 0)
    ld = ImageDraw.Draw(lead)
    for k in range(-30, 60):
        o = k * 64
        ld.line([(l + o, t), (l + o + 1000, t + 1000)], fill=255, width=3)
        ld.line([(r - o, t), (r - o - 1000, t + 1000)], fill=255, width=3)
    sky = tint(sky, (14, 14, 18), scale(lead.filter(ImageFilter.GaussianBlur(0.6)), 0.75))
    # A cracked pane, lower left.
    ck = Image.new("L", img.size, 0)
    cd = ImageDraw.Draw(ck)
    ox, oy = int(l + (r - l) * 0.22), int(b - (b - t - rad) * 0.24)
    for i in range(9):
        a = i * math.tau / 9 + rng.uniform(-0.2, 0.2)
        ln = rng.uniform(40, 150)
        cd.line([(ox, oy), (ox + math.cos(a) * ln, oy + math.sin(a) * ln)], fill=255, width=2)
    for rr in (26, 58):
        cd.ellipse([ox - rr, oy - rr * 0.9, ox + rr, oy + rr * 0.9], outline=150, width=1)
    sky = tint(sky, (150, 160, 184), scale(ck.filter(ImageFilter.GaussianBlur(0.5)), 0.6))
    # A hand on the glass, from the inside: misted, and fading.
    sky = handprint(sky, l + (r - l) * 0.74, b - (b - t - rad) * 0.36, 230, -14, 0.42, (120, 132, 158), smear=4)
    img.paste(sky, mask=gm)
    # The frame and its bars.
    bars = Image.new("L", img.size, 0)
    bdr = ImageDraw.Draw(bars)
    window_bars(bdr, WIN, 255)
    frame = Image.new("L", img.size, 0)
    arch(ImageDraw.Draw(frame), WIN, 255, grow=14)
    edge = ImageChops.subtract(frame, gm)
    bars = ImageChops.lighter(ImageChops.multiply(bars, gm), edge)
    img = tint(img, (20, 15, 15), bars.filter(ImageFilter.GaussianBlur(0.8)))
    # Moonlight spills round the bars and the frame.
    bloom = ImageChops.multiply(gm, ImageChops.invert(bars)).filter(ImageFilter.GaussianBlur(10))
    img = light(img, (96, 110, 140), scale(ImageChops.multiply(bloom, bars.filter(ImageFilter.GaussianBlur(4))), 0.9))
    # The sill, its moonlit lip and its shadow.
    d = ImageDraw.Draw(img)
    d.rectangle([l - 92, b + 2, r + 92, b + 44], fill=(38, 32, 34))
    d.line([(l - 92, b + 2), (r + 92, b + 2)], fill=(108, 114, 132), width=4)
    d.line([(l - 92, b + 44), (r + 92, b + 44)], fill=(10, 8, 9), width=4)
    sh = Image.new("L", img.size, 0)
    ImageDraw.Draw(sh).rectangle([l - 92, b + 46, r + 92, b + 90], fill=150)
    img = tint(img, (5, 4, 5), sh.filter(ImageFilter.GaussianBlur(12)))
    # Cold light on the wall round the window.
    return glow(img, (34, 42, 66), ((l + r) // 2, (t + b) // 2), (1500, 1500), 90, 1.5)


def moonbeam(img):
    """The window's light swept down to the desk: a shaft with the bars'
    shadows in it, and the window's shape lying on the desk."""
    q = 4
    sw, sh = W // q, H // q
    st = glass_mask(WIN, (sw, sh), 1 / q)
    sd = ImageDraw.Draw(st)
    window_bars(sd, tuple(v / q for v in WIN), 0, 1 / q)
    l, t, r, b = WIN
    wcx, wcy = (l + r) / 2, (t + b) / 2
    vx, vy = (LANDING[0] - wcx) / q, (LANDING[1] - wcy) / q
    shaft = Image.new("L", (sw, sh), 0)
    n = 64
    for i in range(n):
        k = i / (n - 1)
        moved = ImageChops.offset(st, int(vx * k), int(vy * k))
        shaft = ImageChops.lighter(shaft, scale(moved, 0.34 * (1 - 0.6 * k)))
    shaft = ImageChops.subtract(shaft, st)  # the glass itself is already lit
    shaft = shaft.filter(ImageFilter.GaussianBlur(5)).resize((W, H), Image.BICUBIC)
    # Not below the desk's edge: the beam stops where it lands.
    stop = Image.new("L", (W, H), 255)
    ImageDraw.Draw(stop).rectangle([0, LANDING[1] + 60, W, H], fill=0)
    shaft = ImageChops.multiply(shaft, stop.filter(ImageFilter.GaussianBlur(60)))
    haze = fractal((W // 8, H // 8), (10, 6), 4).resize((W, H), Image.BICUBIC).point(lambda v: 150 + v * 105 // 255)
    img = light(img, (150, 172, 214), ImageChops.multiply(shaft, haze))
    # The patch on the desk: the window laid flat and skewed.
    s, kx = 0.32, -0.55
    lx, ly = LANDING[0] / q, LANDING[1] / q
    cx, cy = wcx / q, wcy / q
    patch = st.transform((sw, sh), Image.AFFINE, (1, kx / s, cx - lx - kx * ly / s, 0, 1 / s, cy - ly / s), resample=Image.BICUBIC)
    desk_only = Image.new("L", (sw, sh), 0)
    ImageDraw.Draw(desk_only).rectangle([0, DESK_Y // q + 4, sw, sh], fill=255)
    patch = ImageChops.multiply(patch, desk_only).filter(ImageFilter.GaussianBlur(2.5)).resize((W, H), Image.BICUBIC)
    return light(img, (120, 140, 180), scale(patch, 0.30))


# ── shelves ────────────────────────────────────────────────────────────


def board(img, x0, x1, y, brackets=True):
    d = ImageDraw.Draw(img)
    d.rectangle([x0, y, x1, y + 26], fill=(38, 24, 16))
    d.line([(x0, y), (x1, y)], fill=(84, 58, 38), width=3)
    sh = Image.new("L", img.size, 0)
    ImageDraw.Draw(sh).rectangle([x0, y + 26, x1, y + 70], fill=170)
    img = tint(img, (4, 3, 3), sh.filter(ImageFilter.GaussianBlur(14)))
    d = ImageDraw.Draw(img)
    if brackets:
        for bx in (x0 + 60, x1 - 60):
            d.polygon([(bx - 10, y + 26), (bx + 10, y + 26), (bx + 10, y + 120), (bx - 10, y + 26 + 20)], fill=(22, 14, 10))
            d.polygon([(bx - 10, y + 26), (bx + 10, y + 26), (bx - 10, y + 110)], fill=(30, 19, 13))
    return img


def book(d, x, base, w, h, lean, hue):
    pts = [(x, base), (x + w, base), (x + w + lean, base - h), (x + lean, base - h)]
    hue = tuple(int(c * 0.72) for c in hue)
    d.polygon(pts, fill=hue)
    band = tuple(min(255, c + 16) for c in hue)
    for f in (0.12, 0.18, 0.78):
        yy = base - h * f
        d.line([(x + lean * f, yy), (x + w + lean * f, yy)], fill=band, width=3)
    d.line([(x + w + lean, base - h), (x + w, base)], fill=tuple(max(0, c - 10) for c in hue), width=2)


def jar(img, x, base, w, h, liquid, thing):
    """An apothecary jar: murky glass, a stopper, a label, something in it."""
    d = ImageDraw.Draw(img)
    neck = int(w * 0.62)
    d.rounded_rectangle([x, base - h, x + w, base], radius=int(w * 0.22), fill=(10, 12, 11))
    lv = base - int(h * liquid)
    d.rounded_rectangle([x + 6, lv, x + w - 6, base - 6], radius=int(w * 0.18), fill=(19, 24, 16))
    d.line([(x + 10, lv), (x + w - 10, lv)], fill=(38, 46, 30), width=2)
    if thing == "eye":
        ex, ey = x + w * 0.52, lv + (base - lv) * 0.45
        er = w * 0.17
        d.ellipse([ex - er, ey - er, ex + er, ey + er], fill=(54, 48, 38))
        d.ellipse([ex - er * 0.45 + 3, ey - er * 0.45, ex + er * 0.45 + 3, ey + er * 0.45], fill=(12, 10, 8))
        for k in range(4):
            a = rng.uniform(0, math.tau)
            d.line([(ex, ey), (ex + math.cos(a) * er * 1.6, ey + math.sin(a) * er * 1.6 + 10)], fill=(40, 26, 22), width=2)
    elif thing == "curl":
        cx, cy = x + w * 0.5, lv + (base - lv) * 0.55
        pts = [(cx + math.cos(a / 3) * (a * 1.6), cy + math.sin(a / 3) * (a * 1.2)) for a in range(2, 36)]
        d.line(pts, fill=(40, 34, 26), width=7)
    d.rectangle([x + (w - neck) // 2, base - h - 30, x + (w + neck) // 2, base - h + 4], fill=(10, 11, 10))
    d.rectangle([x + (w - neck) // 2 - 8, base - h - 46, x + (w + neck) // 2 + 8, base - h - 28], fill=(28, 19, 12))
    ly = base - int(h * 0.62)
    d.polygon([(x + 16, ly), (x + w - 16, ly + 4), (x + w - 18, ly + int(h * 0.2)), (x + 18, ly + int(h * 0.2) - 3)], fill=(40, 34, 24))
    for k in range(2):
        yy = ly + 14 + k * 16
        d.line([(x + 26, yy), (x + w - 26 - rng.randint(0, 30), yy + 2)], fill=(18, 14, 10), width=3)
    # Glass catches the moon on its right shoulder.
    hl = Image.new("L", img.size, 0)
    ImageDraw.Draw(hl).line([(x + w - 16, base - h + 26), (x + w - 16, base - 30)], fill=200, width=5)
    return tint(img, (110, 122, 142), hl.filter(ImageFilter.GaussianBlur(2.5)))


def skull(img, x, base, s):
    d = ImageDraw.Draw(img)
    bone = (38, 34, 28)
    d.ellipse([x, base - s * 1.1, x + s, base - s * 0.25], fill=bone)  # cranium
    d.rounded_rectangle([x + s * 0.2, base - s * 0.45, x + s * 0.8, base], radius=int(s * 0.12), fill=bone)  # jaw
    hole = (4, 3, 3)
    d.ellipse([x + s * 0.17, base - s * 0.62, x + s * 0.44, base - s * 0.38], fill=hole)
    d.ellipse([x + s * 0.56, base - s * 0.62, x + s * 0.83, base - s * 0.38], fill=hole)
    d.polygon([(x + s * 0.5, base - s * 0.34), (x + s * 0.44, base - s * 0.22), (x + s * 0.56, base - s * 0.22)], fill=hole)
    for k in range(5):
        tx = x + s * (0.28 + k * 0.11)
        d.line([(tx, base - s * 0.16), (tx, base - s * 0.04)], fill=(14, 12, 10), width=3)
    d.arc([x + s * 0.1, base - s * 1.05, x + s * 0.7, base - s * 0.5], 200, 260, fill=(30, 26, 20), width=3)  # a crack
    hl = Image.new("L", img.size, 0)
    ImageDraw.Draw(hl).arc([x, base - s * 1.1, x + s, base - s * 0.25], 290, 20, fill=210, width=6)
    return tint(img, (110, 118, 136), hl.filter(ImageFilter.GaussianBlur(2.5)))


def shelf_right(img):
    x0, x1, y = int(W * 0.745), int(W * 0.995), int(H * 0.555)
    img = board(img, x0, x1, y)
    d = ImageDraw.Draw(img)
    x = x0 + 30
    hues = [(46, 22, 20), (28, 30, 40), (38, 30, 18), (24, 26, 22), (44, 34, 30), (34, 18, 24)]
    for _ in range(4):
        w = rng.randint(30, 50)
        book(d, x, y, w, rng.randint(220, 300), 0, rng.choice(hues))
        x += w + rng.randint(0, 4)
    book(d, x + 4, y, 44, 250, 52, rng.choice(hues))  # leaning
    x += 112
    img = jar(img, x, y, 130, 250, 0.7, "eye")
    x += 160
    img = skull(img, x, y, 140)
    x += 165
    img = jar(img, x, y, 100, 290, 0.55, "curl")
    x += 128
    d = ImageDraw.Draw(img)
    # Two books lying flat, a candle stub on them, never lit.
    d.rectangle([x, y - 40, x + 130, y], fill=(30, 22, 16))
    d.rectangle([x + 8, y - 74, x + 122, y - 40], fill=(42, 20, 18))
    d.rectangle([x + 50, y - 150, x + 78, y - 74], fill=(64, 56, 44))
    d.line([(x + 64, y - 150), (x + 66, y - 166)], fill=(20, 14, 10), width=3)
    return img


def shelf_left(img):
    x0, x1, y = int(W * 0.012), int(W * 0.21), int(H * 0.135)
    img = board(img, x0, x1, y)
    d = ImageDraw.Draw(img)
    x = x0 + 40
    hues = [(40, 20, 18), (24, 26, 34), (34, 26, 16), (30, 18, 22)]
    for _ in range(4):
        w = rng.randint(28, 46)
        book(d, x, y, w, rng.randint(170, 230), 0, rng.choice(hues))
        x += w + rng.randint(0, 3)
    book(d, x + 2, y, 40, 200, -46, rng.choice(hues))
    x += 90
    img = jar(img, x, y, 120, 190, 0.5, "curl")
    x += 160
    d = ImageDraw.Draw(img)
    # A bottle, a stopper, nothing on the label.
    d.rounded_rectangle([x, y - 160, x + 70, y], radius=18, fill=(18, 22, 20))
    d.rectangle([x + 24, y - 220, x + 46, y - 156], fill=(18, 22, 20))
    d.rectangle([x + 20, y - 236, x + 50, y - 218], fill=(40, 28, 18))
    d.rectangle([x + 10, y - 110, x + 60, y - 60], fill=(36, 31, 23))
    return img


# ── the desk ───────────────────────────────────────────────────────────


def desk(img):
    """Walnut planks running along the desk; one has a face in its grain."""
    seams = [0, 130, 300, 740, H - DESK_Y]
    for i in range(len(seams) - 1):
        y0, y1 = DESK_Y + seams[i], DESK_Y + seams[i + 1]
        ph = y1 - y0
        knots = []
        for _ in range(rng.randint(1, 2)):
            knots.append((rng.randint(300, W - 300), rng.randint(10, ph - 10), rng.randint(14, 30), rng.uniform(1.5, 2.6)))
        if y0 <= FACE[1] < y1:
            fx, fy = FACE[0], FACE[1] - y0
            knots = [k for k in knots if abs(k[0] - fx) > 500]
            knots += [(fx - 86, fy - 62, 42, 5.0), (fx + 86, fy - 66, 42, 5.0), (fx, fy + 90, 36, 3.6)]
        tex = wood(W, ph, (12, 6, 4), (76, 46, 26), rng.uniform(30, 46), rng.uniform(2.5, 4.0), knots, fiber=0.3, cells=(4, 2))
        img.paste(tex, (0, y0))
    d = ImageDraw.Draw(img)
    for s in seams[1:-1]:
        y = DESK_Y + s
        d.line([(0, y), (W, y)], fill=(6, 3, 2), width=3)
        d.line([(0, y + 3), (W, y + 3)], fill=(44, 28, 18), width=1)
    # The face: hollows where the knots are, a mouth that's a split.
    fx, fy = FACE
    face = Image.new("L", img.size, 0)
    fd = ImageDraw.Draw(face)
    fd.ellipse([fx - 120, fy - 84, fx - 52, fy - 40], fill=200)
    fd.ellipse([fx + 52, fy - 88, fx + 120, fy - 44], fill=200)
    fd.ellipse([fx - 50, fy + 66, fx + 44, fy + 112], fill=170)
    img = tint(img, (5, 2, 2), scale(face.filter(ImageFilter.GaussianBlur(12)), 0.85))
    # Scratches and old rings in the varnish.
    sc = Image.new("L", img.size, 0)
    sd = ImageDraw.Draw(sc)
    for _ in range(90):
        x, y = rng.randint(0, W), rng.randint(DESK_Y + 20, H)
        a = rng.gauss(0, 0.25)
        ln = rng.uniform(30, 180)
        sd.line([(x, y), (x + math.cos(a) * ln, y + math.sin(a) * ln * 0.3)], fill=rng.randint(60, 140), width=1)
    for cx, cy, rr in ((int(W * 0.30), DESK_Y + 230, 90), (int(W * 0.93), DESK_Y + 420, 76)):
        sd.ellipse([cx - rr, cy - rr * 0.36, cx + rr, cy + rr * 0.36], outline=90, width=3)
    img = tint(img, (90, 66, 46), scale(sc.filter(ImageFilter.GaussianBlur(0.5)), 0.5))
    # The front falls into shadow; the back edge catches light.
    hh = H - DESK_Y
    shade = Image.linear_gradient("L").resize((W, hh)).point(lambda v: int(v * 0.74))
    region = img.crop((0, DESK_Y, W, H))
    region = Image.composite(solid((6, 4, 3), (W, hh)), region, shade)
    img.paste(region, (0, DESK_Y))
    d = ImageDraw.Draw(img)
    d.line([(0, DESK_Y), (W, DESK_Y)], fill=(86, 56, 34), width=5)
    d.line([(0, DESK_Y + 5), (W, DESK_Y + 5)], fill=(18, 11, 7), width=6)
    return img


def inkwell(img):
    """An inkwell and a quill, right of the book, in the moon's path."""
    x, y = int(W * 0.885), DESK_Y + 150
    d = ImageDraw.Draw(img)
    d.ellipse([x - 104, y - 8, x + 70, y + 24], fill=(7, 5, 4))  # its shadow
    d.rounded_rectangle([x - 74, y - 96, x + 74, y + 12], radius=40, fill=(9, 10, 13))
    d.rectangle([x - 30, y - 128, x + 30, y - 90], fill=(9, 10, 13))
    d.ellipse([x - 38, y - 140, x + 38, y - 118], fill=(22, 20, 18))
    d.ellipse([x - 26, y - 136, x + 26, y - 122], fill=(3, 2, 3))
    # The quill: a feather out of the ink, bent, its vane ragged.
    q = Image.new("L", img.size, 0)
    qd = ImageDraw.Draw(q)
    sx, sy = x + 4, y - 128
    pts = [(sx - t * 2.2 - (t / 30) ** 2.4, sy - t * 4.6) for t in range(0, 78)]
    left, right = [], []
    for i in range(18, 78):
        px, py = pts[i]
        span = 46 * math.sin(math.pi * (i - 16) / 64) ** 0.7
        left.append((px - span * rng.uniform(0.85, 1.0), py + span * 0.5))
        right.append((px + span * 0.6 * rng.uniform(0.8, 1.0), py + span * 0.3))
    qd.polygon(left + [pts[77]] + right[::-1], fill=150)
    for i in range(20, 76, 3):  # splits in the vane
        px, py = pts[i]
        qd.line([(px, py), (px - 50, py + 30)], fill=40, width=2)
    qd.line(pts, fill=255, width=4)
    img = tint(img, (84, 78, 70), q.filter(ImageFilter.GaussianBlur(0.9)))
    hl = Image.new("L", img.size, 0)
    hd = ImageDraw.Draw(hl)
    hd.arc([x - 74, y - 96, x + 74, y + 12], 300, 20, fill=210, width=5)
    hd.line(pts[20:76], fill=120, width=2)
    return tint(img, (120, 132, 156), hl.filter(ImageFilter.GaussianBlur(2)))


def candle(img):
    """A stub of candle in a brass holder, and its glow on wall and desk."""
    cx, cy = CANDLE
    img = glow(img, (230, 110, 40), (cx, cy + 80), (3400, 2800), 124, 1.3)
    img = glow(img, (255, 150, 70), (cx, cy), (1120, 1120), 150, 1.8)
    d = ImageDraw.Draw(img)
    dish_y = DESK_Y + 180
    d.ellipse([cx - 250, dish_y - 30, cx + 250, dish_y + 70], fill=(10, 6, 4))  # shadow on the desk
    d.ellipse([cx - 240, dish_y - 52, cx + 240, dish_y + 52], fill=(84, 58, 28))
    d.ellipse([cx - 200, dish_y - 44, cx + 200, dish_y + 24], fill=(126, 90, 44))
    d.ellipse([cx - 150, dish_y - 32, cx + 150, dish_y + 10], fill=(150, 112, 58))
    d.ellipse([cx + 210, dish_y - 20, cx + 300, dish_y + 24], outline=(96, 68, 32), width=12)  # the finger ring
    # A pool of wax in the dish, and the wax, dripped.
    d.ellipse([cx - 110, dish_y - 30, cx + 90, dish_y - 2], fill=(200, 176, 132))
    d.rectangle([cx - 68, cy + 80, cx + 68, dish_y - 14], fill=(196, 170, 128))
    d.ellipse([cx - 68, cy + 56, cx + 68, cy + 104], fill=(226, 200, 150))
    d.ellipse([cx - 40, cy + 66, cx + 40, cy + 94], fill=(210, 184, 138))  # the well round the wick
    for _ in range(7):
        dx = rng.randint(-62, 52)
        ln = rng.randint(60, 220)
        d.rounded_rectangle([cx + dx, cy + 88, cx + dx + 16, cy + 88 + ln], radius=8, fill=(214, 188, 142))
        d.ellipse([cx + dx - 3, cy + 80 + ln, cx + dx + 19, cy + 100 + ln], fill=(214, 188, 142))
    # Round it: the wax's sides turn away from the light.
    sh = Image.new("L", img.size, 0)
    sd = ImageDraw.Draw(sh)
    sd.rectangle([cx + 22, cy + 80, cx + 80, dish_y - 14], fill=120)
    sd.rectangle([cx - 80, cy + 80, cx - 44, dish_y - 14], fill=60)
    img = tint(img, (70, 50, 30), sh.filter(ImageFilter.GaussianBlur(16)))
    d = ImageDraw.Draw(img)
    d.line([(cx, cy + 60), (cx + 4, cy + 34)], fill=(30, 20, 14), width=5)
    flame = Image.new("L", img.size, 0)
    fd = ImageDraw.Draw(flame)
    fd.ellipse([cx - 26, cy - 68, cx + 26, cy + 44], fill=255)
    fd.polygon([(cx - 22, cy - 20), (cx + 22, cy - 20), (cx + 6, cy - 124)], fill=255)
    img.paste((255, 214, 140), mask=flame.filter(ImageFilter.GaussianBlur(7)))
    core = Image.new("L", img.size, 0)
    ImageDraw.Draw(core).ellipse([cx - 10, cy - 16, cx + 10, cy + 32], fill=230)
    img.paste((255, 250, 228), mask=core.filter(ImageFilter.GaussianBlur(5)))
    blue = Image.new("L", img.size, 0)
    ImageDraw.Draw(blue).ellipse([cx - 12, cy + 18, cx + 12, cy + 44], fill=150)
    return tint(img, (90, 110, 200), blue.filter(ImageFilter.GaussianBlur(4)))


# ── cobwebs and the spider ─────────────────────────────────────────────


def cobweb(img, corner, size, sx, sy, alpha=0.62, spokes=13, a0=4, a1=86):
    """A web in a corner (sx, sy = the directions into the room): radials,
    a sagging spiral with gaps, loose strands, dust caught on it."""
    k = 2
    pad = int(size * 0.35)
    box = int(size * 1.0) + pad
    lay, d = ss_layer((box, box), k)
    ox, oy = (0 if sx > 0 else box), (0 if sy > 0 else box)
    angles = [math.radians(a0 + i * ((a1 - a0) / (spokes - 1)) + rng.uniform(-2.5, 2.5)) for i in range(spokes)]
    lens = [size * rng.uniform(0.8, 1.0) for _ in angles]

    def pt(a, r):
        return ((ox + sx * r * math.cos(a)) * k, (oy + sy * r * math.sin(a)) * k)

    for a, ln in zip(angles, lens):
        d.line([pt(a, 0), pt(a, ln)], fill=230, width=2 * k)
    r = size * 0.06
    while r < size * 0.95:
        for i in range(spokes - 1):
            if r > min(lens[i], lens[i + 1]) or rng.random() < 0.12:
                continue  # torn
            a, b = angles[i], angles[i + 1]
            sag = rng.uniform(0.04, 0.09)
            seg = [pt(a + (b - a) * u / 6, r * (1 - sag * math.sin(math.pi * u / 6))) for u in range(7)]
            d.line(seg, fill=rng.randint(140, 195), width=k)
        r *= rng.uniform(1.07, 1.13)
    for _ in range(5):  # loose strands, hanging
        a = rng.choice(angles)
        r0 = size * rng.uniform(0.3, 0.9)
        x0, y0 = pt(a, r0)
        drop = size * rng.uniform(0.15, 0.5) * k
        sway = rng.uniform(-40, 40) * k
        pts = [(x0 + sway * math.sin(t / 10 * math.pi / 2), y0 + drop * t / 10) for t in range(11)]
        d.line(pts, fill=150, width=k)
    for _ in range(int(size / 40)):  # dust caught in the threads
        a = rng.choice(angles)
        x0, y0 = pt(a, size * rng.uniform(0.05, 0.9))
        s = rng.uniform(1.0, 2.6) * k
        d.ellipse([x0 - s, y0 - s, x0 + s, y0 + s], fill=200)
    web = ss_done(lay, (box, box))
    m = Image.new("L", img.size, 0)
    m.paste(scale(web, alpha), (corner[0] - ox, corner[1] - oy))
    return tint(img, (206, 200, 192), m)


def spider(img, x, y, top):
    k = 2
    box = 140
    lay, d = ss_layer((box, box), k)
    c = box // 2
    d.ellipse([(c - 14) * k, (c - 10) * k, (c + 14) * k, (c + 22) * k], fill=255)  # abdomen
    d.ellipse([(c - 9) * k, (c - 26) * k, (c + 9) * k, (c - 6) * k], fill=255)
    for s in (-1, 1):
        for a in (-60, -25, 10, 45):
            r = math.radians(a)
            kx, ky = c + s * 34 * math.cos(r), c - 14 + 34 * math.sin(r) - 12
            fx, fy = kx + s * 18, ky + 30
            d.line([(c * k, (c - 14) * k), (kx * k, ky * k), (fx * k, fy * k)], fill=240, width=3 * k)
    body = ss_done(lay, (box, box))
    m = Image.new("L", img.size, 0)
    ImageDraw.Draw(m).line([(x, top), (x, y - 20)], fill=110, width=2)
    m.paste(body, (x - c, y - c), body)
    img = tint(img, (8, 6, 7), m)
    hl = Image.new("L", img.size, 0)
    ImageDraw.Draw(hl).arc([x - 13, y - 9, x + 13, y + 21], 300, 30, fill=150, width=3)
    return tint(img, (90, 96, 112), hl.filter(ImageFilter.GaussianBlur(1)))


# ── air ────────────────────────────────────────────────────────────────


def mist(img, calm):
    """Low mist over the desk and a haze in the room."""
    q = 4
    fog = Image.new("L", (W // q, H // q), 0)
    d = ImageDraw.Draw(fog)
    for _ in range(80):
        x = rng.uniform(-50, W // q + 50)
        y = rng.uniform(H // q * 0.45, H // q * 1.05)
        rx, ry = rng.uniform(30, 110), rng.uniform(6, 18)
        d.ellipse([x - rx, y - ry, x + rx, y + ry], fill=rng.randint(20, 46))
    for _ in range(16):
        x = rng.uniform(0, W // q)
        y = rng.uniform(0, H // q * 0.5)
        r = rng.uniform(40, 90)
        d.ellipse([x - r, y - r * 0.5, x + r, y + r * 0.5], fill=rng.randint(6, 14))
    fog = fog.filter(ImageFilter.GaussianBlur(14)).resize((W, H), Image.BICUBIC)
    wisps = fractal((W // 4, H // 4), (6, 16), 5, 0.6).resize((W, H), Image.BICUBIC)
    fog = ImageChops.multiply(fog, wisps.point(lambda v: min(255, 60 + v)))
    fog = ImageChops.multiply(fog, ImageChops.invert(scale(calm, 0.5)))
    return Image.composite(solid((150, 160, 190)), img, fog)


def dust(img):
    """Motes in the candle light and in the moonbeam."""
    warm = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(warm)
    cx, cy = CANDLE
    for _ in range(420):
        a = rng.uniform(0, math.tau)
        r = abs(rng.gauss(0, 660))
        x, y = cx + r * math.cos(a) * 1.4, cy - 120 + r * math.sin(a)
        s = rng.choice([1.5, 2, 2.5, 3, 4])
        d.ellipse([x - s, y - s, x + s, y + s], fill=rng.randint(60, 170))
    img = tint(img, (255, 220, 170), warm.filter(ImageFilter.GaussianBlur(1.0)))
    cold = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(cold)
    l, t, r, b = WIN
    for _ in range(380):
        k = rng.random()
        x = rng.uniform(l, r) + (LANDING[0] - (l + r) / 2) * k
        y = rng.uniform(t + 200, b) + (LANDING[1] - (t + b) / 2) * k
        s = rng.choice([1.5, 2, 2.5, 3, 4])
        d.ellipse([x - s, y - s, x + s, y + s], fill=rng.randint(50, 150))
    return tint(img, (210, 222, 255), cold.filter(ImageFilter.GaussianBlur(1.0)))


def watcher(img):
    """Something in the dark corner, top left, that isn't a knot."""
    m = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(m)
    x, y = int(W * 0.226), int(H * 0.082)
    for dx in (0, 44):
        d.ellipse([x + dx - 7, y - 4, x + dx + 7, y + 4], fill=255)
    img = light(img, (60, 54, 20), scale(m.filter(ImageFilter.GaussianBlur(14)), 0.6))
    return light(img, (150, 140, 70), scale(m.filter(ImageFilter.GaussianBlur(1.6)), 0.75))


def calm_mask():
    """Where the book sits: the middle, full height."""
    m = Image.new("L", (W, H), 0)
    m.paste(radial((int(W * 0.62), int(H * 1.5)), 255, 0, 1.6), (int(W * 0.19), int(-H * 0.25)))
    return m


def finish(img, calm):
    # The middle: calmer and darker for reading over.
    img = Image.composite(solid((5, 4, 5)), img, scale(calm, 0.6))
    # A darker room: midtones sink, the lights stay.
    img = img.point([int(255 * (i / 255) ** 1.12) for i in range(256)] * 3)
    # The dark closes in at the edges, most in the corners.
    g = Image.radial_gradient("L").resize((W, H), Image.BICUBIC)
    img = Image.composite(solid((2, 1, 3)), img, g.point([int(185 * (i / 255) ** 2.2) for i in range(256)]))
    # Film grain, a couple of pixels across.
    g = Image.blend(noise(W // 2, H // 2), noise(W // 2, H // 2), 0.5)
    g = ImageOps.autocontrast(g, cutoff=0.2).resize((W, H), Image.BICUBIC)
    return Image.composite(ImageChops.add(img, solid((6, 6, 6))), ImageChops.subtract(img, solid((6, 6, 6))), g)


def main(out):
    calm = calm_mask()
    img = wall()
    img = stain(img, int(W * 0.12), CORNICE + 60, 380, 150, 420)
    img = stain(img, int(W * 0.962), int(H * 0.08), 150, 230, 380)
    img = stain(img, int(W * 0.47), CORNICE + 40, 300, 110, 260)
    img = cracks(img)
    img = tallies(img)
    img = handprint(img, W * 0.218, H * 0.272, 400, 12, 0.42, (58, 10, 10))
    img = handprint(img, W * 0.252, H * 0.322, 330, -8, 0.30, (58, 10, 10))
    img = drips(img)
    img = window(img)
    img = shelf_left(img)
    img = shelf_right(img)
    img = desk(img)
    img = inkwell(img)
    img = moonbeam(img)
    img = candle(img)
    img = cobweb(img, (W, 0), 720, -1, 1)
    img = cobweb(img, (0, 0), 700, 1, 1)
    l, t, r, b = WIN
    img = cobweb(img, (r + 54, b + 2), 210, -1, -1, 0.5, 8)  # in the window's reveal, by the sill
    img = cobweb(img, (int(W * 0.21) - 60, int(H * 0.135) + 26), 170, -1, 1, 0.45, 7)  # under the left shelf
    img = cobweb(img, (int(W * 0.995) - 60, int(H * 0.555) + 26), 190, -1, 1, 0.45, 7)  # under the right shelf
    img = spider(img, int(W * 0.955), int(H * 0.215), 0)
    img = watcher(img)
    img = mist(img, calm)
    img = dust(img)
    img = finish(img, calm)
    if out.endswith(".png"):
        img.save(out, "PNG")
    else:
        img.save(out, "WEBP", quality=int(QUALITY), method=6)


QUALITY = 92  # about 0.8 MB: dark gradients without banding, grain kept

if __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        sys.exit("usage: haunted-background-2.py <out.webp> [quality]")
    if len(sys.argv) == 3:
        QUALITY = int(sys.argv[2])
    main(sys.argv[1])
