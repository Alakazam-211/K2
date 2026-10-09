#!/usr/bin/env python3
"""Draw the second `haunted` Zen background (Rosson 2026-10-08: "not
haunted enough"). The first study (haunted-background.py) plus: a cold
moonbeam from a high window with dust hanging in it, tally marks scratched
into the wall, smeared handprints, ink weeping down from the cornice, a
third web with its spider, a face you only half see in the desk's grain,
and a heavier dark at the edges. Procedural, seeded, no outside art:

    python3 scripts/zen-themes/haunted-background-2.py \
        crates/k2-core/src/zen/themes/haunted-background-2.webp

The middle stays calm: the Diary's book sits there. What moves (fog, the
candle guttering, a shape passing, the breathing dark) is drawn by the
Diary widget behind its book; a still image can't move.
"""

import importlib.util
import math
import os
import random
import sys

from PIL import Image, ImageDraw, ImageFilter

_spec = importlib.util.spec_from_file_location("hb1", os.path.join(os.path.dirname(os.path.abspath(__file__)), "haunted-background.py"))
hb1 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(hb1)

W, H, DESK_Y, CANDLE = hb1.W, hb1.H, hb1.DESK_Y, hb1.CANDLE
rng = random.Random(20261031)


def tint(img, color, mask):
    img.paste(color, mask=mask)
    return img


def moonbeam(img):
    """Cold light slanting from a high window, top right, onto the desk."""
    layer = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(layer)
    d.polygon([(int(W * 0.60), -10), (int(W * 0.71), -10), (int(W * 0.98), DESK_Y + 160), (int(W * 0.70), DESK_Y + 160)], fill=46)
    layer = layer.filter(ImageFilter.GaussianBlur(28))
    img = tint(img, (150, 172, 214), layer)
    # The window's cross, barely there, where the beam starts.
    frame = Image.new("L", img.size, 0)
    fd = ImageDraw.Draw(frame)
    fd.rectangle([int(W * 0.60), -10, int(W * 0.71), int(H * 0.06)], outline=60, width=6)
    fd.line([(int(W * 0.655), -10), (int(W * 0.655), int(H * 0.06))], fill=60, width=5)
    img = tint(img, (120, 136, 170), frame.filter(ImageFilter.GaussianBlur(2)))
    # Dust hanging in the beam.
    motes = Image.new("L", img.size, 0)
    md = ImageDraw.Draw(motes)
    for _ in range(170):
        k = rng.random()
        y = k * (DESK_Y + 120)
        left = W * 0.60 + (W * 0.70 - W * 0.60) * k
        right = W * 0.71 + (W * 0.98 - W * 0.71) * k
        x = rng.uniform(left, right)
        s = rng.choice([1, 1, 1.5, 2, 2.5])
        md.ellipse([x - s, y - s, x + s, y + s], fill=rng.randint(50, 150))
    return tint(img, (210, 222, 255), motes.filter(ImageFilter.GaussianBlur(0.8)))


def tallies(img):
    """Days counted by someone who never left: scratched into the wall."""
    layer = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(layer)
    x0, y0 = int(W * 0.045), int(H * 0.17)
    for row in range(4):
        x = x0 + rng.randint(-8, 8)
        y = y0 + row * 64
        for group in range(rng.randint(3, 5)):
            marks = 5 if (row, group) != (3, 0) else rng.randint(1, 3)
            for m in range(min(marks, 4)):
                tilt = rng.uniform(-5, 5)
                d.line([(x + m * 11, y), (x + m * 11 + tilt, y + 44)], fill=rng.randint(120, 190), width=3)
            if marks == 5:
                d.line([(x - 6, y + 34), (x + 42, y + 8)], fill=170, width=3)
            x += 64
    layer = layer.filter(ImageFilter.GaussianBlur(0.6))
    return tint(img, (196, 172, 150), layer)


def handprint(img, cx, cy, scale, angle, alpha):
    """A smeared hand, palm and splayed fingers, dragged a little down."""
    size = int(260 * scale)
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
    # The smear: the hand slid down the wall.
    smear = hand.copy()
    for k in range(1, 8):
        hand = Image.composite(hand, smear.transform(smear.size, Image.AFFINE, (1, 0, 0, 0, 1, -k * size * 0.03)), hand)
    hand = hand.rotate(angle, resample=Image.BICUBIC, expand=True).filter(ImageFilter.GaussianBlur(2.2))
    mask = Image.new("L", img.size, 0)
    mask.paste(hand.point(lambda v: int(v * alpha)), (int(cx - hand.width / 2), int(cy - hand.height / 2)))
    return tint(img, (58, 10, 10), mask)


def drips(img):
    """Ink weeping from the cornice."""
    layer = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(layer)
    for x in (W * 0.05, W * 0.215, W * 0.25, W * 0.665, W * 0.69):
        x = int(x + rng.randint(-20, 20))
        length = rng.randint(int(H * 0.08), int(H * 0.3))
        w = rng.uniform(2.5, 5)
        pts = [(x + math.sin(y / 37) * 1.5, y) for y in range(0, length, 6)]
        d.line(pts, fill=230, width=int(w))
        d.ellipse([x - w * 1.4, length - w, x + w * 1.4, length + w * 2.2], fill=240)
    d.rectangle([0, 0, W, 10], fill=200)
    return tint(img, (60, 6, 9), layer.filter(ImageFilter.GaussianBlur(1)))


def spider(img, x, y):
    layer = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(layer)
    d.line([(x, 0), (x, y - 10)], fill=120, width=1)
    d.ellipse([x - 9, y - 10, x + 9, y + 10], fill=230)
    d.ellipse([x - 5, y - 18, x + 5, y - 8], fill=230)
    for s in (-1, 1):
        for i, a in enumerate((-50, -15, 15, 45)):
            r = math.radians(a)
            kx, ky = x + s * 18 * math.cos(r), y + 18 * math.sin(r) - 6
            d.line([(x, y), (kx, ky), (kx + s * 10, ky + 14)], fill=220, width=2)
    return tint(img, (10, 8, 8), layer.filter(ImageFilter.GaussianBlur(0.6)))


def grain_face(img):
    """In the desk's grain, low on the right: a face, if you look."""
    cx, cy = int(W * 0.21), int(DESK_Y + (H - DESK_Y) * 0.34)
    layer = Image.new("L", img.size, 0)
    d = ImageDraw.Draw(layer)
    d.ellipse([cx - 120, cy - 40, cx - 52, cy - 6], fill=150)  # eyes, hollow
    d.ellipse([cx + 52, cy - 44, cx + 124, cy - 8], fill=150)
    d.ellipse([cx - 52, cy + 40, cx + 46, cy + 96], fill=120)  # the mouth, open
    for r in range(150, 230, 14):  # the grain bends around it
        d.ellipse([cx - r * 1.5, cy - r * 0.55, cx + r * 1.5, cy + r * 0.7], outline=70, width=2)
    return tint(img, (6, 3, 2), layer.filter(ImageFilter.GaussianBlur(7)))


def deeper_dark(img):
    mask = hb1.radial((W, H), 0, 215, 2.0)
    return Image.composite(Image.new("RGB", (W, H), (2, 1, 3)), img, mask)


def main(out):
    hb1.rng.seed(20261008)  # the first study, exactly as it was drawn
    img = Image.new("RGB", (W, H), (12, 9, 12))
    img = hb1.wall(img)
    img = tallies(img)
    img = handprint(img, W * 0.30, H * 0.30, 0.85, 12, 0.45)
    img = handprint(img, W * 0.345, H * 0.36, 0.7, -8, 0.32)
    img = handprint(img, W * 0.885, H * 0.62 - 40, 0.6, 24, 0.28)
    img = drips(img)
    img = hb1.shelf(img)
    img = hb1.desk(img)
    img = moonbeam(img)
    img = hb1.candle(img)
    img = grain_face(img)
    img = hb1.cobweb(img, (W, 0), 470, True)
    img = hb1.cobweb(img, (0, 0), 340, False)
    img = spider(img, int(W * 0.775), int(H * 0.25))
    img = hb1.mist(img)
    img = hb1.dust(img)
    img = deeper_dark(img)
    img.save(out, "WEBP", quality=74, method=6)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: haunted-background-2.py <out.webp>")
    main(sys.argv[1])
