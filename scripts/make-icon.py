"""Draws rexplorer's icon (a flat orange crab with a bold white R on its body) into crates/app/app.ico.

    python -I scripts/make-icon.py [dest.ico]

Standard library only. From 32 px up, shapes are signed distance fields, so every size is drawn directly with
smooth edges; 32-48 drop the legs and eyes and thicken the R. 16, 20 and 24 are hand-placed pixel maps on the
pixel grid: body, claw stubs and the R only. Sizes 16-64 are 32-bit BMP entries, 256 is PNG.
"""
import math
import struct
import sys
import zlib

SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
DARK = (0x2A, 0x14, 0x0E)
DEEP = (0xCE, 0x42, 0x2B)
BRIGHT = (0xF7, 0x4C, 0x00)
WHITE = (0xFF, 0xFF, 0xFF)

# Unit-square coordinates. Claws, arms, legs and eyes are given for the left half; the right half mirrors them.
LARGE = dict(
    body=(0.13, 0.40, 0.87, 0.93, 0.22),  # x0, y0, x1, y1, corner radius
    claw=(0.19, 0.18, 0.12, 0.35, -1.0, 22),  # centre x, y, radius, opening direction x, y, notch half-angle
    arm=(0.30, 0.50, 0.20, 0.22, 0.05),  # capsule: a, b, radius
    legs=[(0.16, 0.63, 0.07, 0.70, 0.028), (0.16, 0.76, 0.07, 0.85, 0.028)],
    eye=(0.41, 0.42, 0.41, 0.32, 0.02, 0.052),  # stalk from (x, y) to (x, y), stalk radius, eye radius
    r=(0.35, 0.47, 0.66, 0.87, 0.08, 0.71),  # R box x0, y0, x1, y1, stroke, bowl bottom
)
# 32 to 48 px: no legs or eyes, a heavier R and a wider claw notch.
MEDIUM = dict(LARGE, legs=[], eye=None, claw=(0.19, 0.18, 0.125, 0.35, -1.0, 28), r=(0.34, 0.47, 0.67, 0.87, 0.098, 0.72))

# Under 32 px the icon is drawn pixel by pixel: B bright, D deep, W white, K dark, . transparent.
PIXEL_MAPS = {
    16: [
        "BB.B........B.BB",
        "BB.B........B.BB",
        "BBBB........BBBB",
        ".BB..........BB.",
        "..BB........BB..",
        "..BBBBBBBBBBBB..",
        ".BBBBWWWWWBBBBB.",
        "BBBBBWWBBWWBBBBB",
        "BBBBBWWBBWWBBBBB",
        "BBBBBWWBBWWBBBBB",
        "BBBBBWWWWWBBBBBB",
        "BBBBBWWBWWBBBBBB",
        "BBBBBWWBBWWBBBBB",
        "BBBBBWWBBWWBBBBB",
        ".BBBBWWBBWWBBBB.",
        "..BBBBBBBBBBBB..",
    ],
    20: [
        "BB..B..........B..BB",
        "BB.BB..........BB.BB",
        "BBBBB..........BBBBB",
        ".BBB............BBB.",
        "..BB............BB..",
        "..BBB..........BBB..",
        "...BB..........BB...",
        "..BBBBBBBBBBBBBBBB..",
        ".BBBBBWWWWWWWBBBBBB.",
        "BBBBBBWWWWWWWWBBBBBB",
        "BBBBBBWWWBBBWWBBBBBB",
        "BBBBBBWWWBBBWWBBBBBB",
        "BBBBBBWWWWWWWWBBBBBB",
        "BBBBBBWWWWWWWBBBBBBB",
        "BBBBBBWWWBWWBBBBBBBB",
        "BBBBBBWWWBBWWBBBBBBB",
        "BBBBBBWWWBBWWWBBBBBB",
        "BBBBBBWWWBBBWWBBBBBB",
        ".BBBBBWWWBBBWWBBBBB.",
        "..BBBBBBBBBBBBBBBB..",
    ],
    24: [
        "BB...B............B...BB",
        "BBB.BB............BB.BBB",
        "BBBBBB............BBBBBB",
        "BBBBBB............BBBBBB",
        ".BBBB..............BBBB.",
        "..BBB..............BBB..",
        "...BB..............BB...",
        "...BBB............BBB...",
        "...BBBBBBBBBBBBBBBBBB...",
        "..BBBBBBBBBBBBBBBBBBBB..",
        ".BBBBBBWWWWWWWWBBBBBBBB.",
        ".BBBBBBWWWWWWWWWBBBBBBB.",
        ".BBBBBBWWWBBBBWWWBBBBBB.",
        ".BBBBBBWWWBBBBWWWBBBBBB.",
        ".BBBBBBWWWBBBBWWWBBBBBB.",
        ".BBBBBBWWWWWWWWWBBBBBBB.",
        ".BBBBBBWWWWWWWWBBBBBBBB.",
        ".BBBBBBWWWBBWWWBBBBBBBB.",
        ".BBBBBBWWWBBBWWWBBBBBBB.",
        ".BBBBBBWWWBBBBWWWBBBBBB.",
        ".BBBBBBWWWBBBBWWWBBBBBB.",
        ".BBBBBBWWWBBBBWWWBBBBBB.",
        "..BBBBBBBBBBBBBBBBBBBB..",
        "...BBBBBBBBBBBBBBBBBB...",
    ],
}
for _n, _rows in PIXEL_MAPS.items():
    assert len(_rows) == _n and all(len(r) == _n for r in _rows), _n
PALETTE = {"K": (*DARK, 255), "B": (*BRIGHT, 255), "D": (*DEEP, 255), "W": (*WHITE, 255), ".": (0, 0, 0, 0)}


def params(n):
    return LARGE if n >= 64 else MEDIUM


def rrect(x, y, x0, y0, x1, y1, r):
    hx, hy = (x1 - x0) / 2, (y1 - y0) / 2
    qx, qy = abs(x - (x0 + x1) / 2) - hx + r, abs(y - (y0 + y1) / 2) - hy + r
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r


def capsule(x, y, ax, ay, bx, by, r):
    px, py, dx, dy = x - ax, y - ay, bx - ax, by - ay
    t = max(0.0, min(1.0, (px * dx + py * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - t * dx, py - t * dy) - r


def claw(x, y, cx, cy, r, dx, dy, half):
    """A disc with a V notch cut from the apex (just behind the centre) out along the opening direction."""
    k = math.hypot(dx, dy)
    dx, dy = dx / k, dy / k
    ax, ay = cx - dx * r * 0.15, cy - dy * r * 0.15
    h = math.radians(half + 90)
    c, s = math.cos(h), math.sin(h)
    n1 = (dx * c - dy * s, dx * s + dy * c)
    n2 = (dx * c + dy * s, -dx * s + dy * c)
    wedge = max((x - ax) * n1[0] + (y - ay) * n1[1], (x - ax) * n2[0] + (y - ay) * n2[1])
    return max(math.hypot(x - cx, y - cy) - r, -wedge)


def letter_r(x, y, x0, y0, x1, y1, s, yb):
    """A geometric sans-serif R: a stem, a half-stadium bowl and a straight diagonal leg, all of stroke s."""
    stem = rrect(x, y, x0, y0, x0 + s, y1, 0.0)
    rad = (yb - y0) / 2
    outer = max(rrect(x, y, x0 - rad, y0, x1, yb, rad), x0 - x)
    hole = rrect(x, y, x0 - rad, y0 + s, x1 - s, yb - s, rad - s)
    bowl = max(outer, -hole)
    # Leg: a band of width s from the bowl's lower middle to the bottom-right corner, cut flat at both ends.
    ax, ay, bx, by = x0 + 0.42 * (x1 - x0), yb - s / 2, x1 - s * 0.55, y1
    dx, dy = bx - ax, by - ay
    band = abs((x - ax) * dy - (y - ay) * dx) / math.hypot(dx, dy) - s / 2
    leg = max(band, yb - s - y, y - y1)
    return min(stem, bowl, leg)


def layers(x, y, p):
    """Back-to-front (distance, colour) pairs."""
    f = min(x, 1 - x)  # fold the right half onto the left
    out = []
    if p["legs"]:
        out.append((min(capsule(f, y, *leg) for leg in p["legs"]), DEEP))
    shell = min(rrect(x, y, *p["body"]), capsule(f, y, *p["arm"]), claw(f, y, *p["claw"]))
    if p["eye"]:
        ax, ay, bx, by, sr, er = p["eye"]
        shell = min(shell, capsule(f, y, ax, ay, bx, by, sr), math.hypot(f - bx, y - by) - er)
    out.append((shell, BRIGHT))
    if p["eye"]:
        out.append((math.hypot(f - bx, y - by) - er * 0.68, WHITE))  # an orange rim keeps the eye on a light background
        out.append((math.hypot(f - bx, y - by) - er * 0.36, DARK))
    out.append((letter_r(x, y, *p["r"]), WHITE))
    return out


def pixels(n):
    """Rows of (r, g, b, a), top to bottom."""
    if n in PIXEL_MAPS:
        return [[PALETTE[c] for c in row] for row in PIXEL_MAPS[n]]
    p = params(n)
    rows = []
    for j in range(n):
        row = []
        for i in range(n):
            x, y = (i + 0.5) / n, (j + 0.5) / n
            acc, alpha = [0.0, 0.0, 0.0], 0.0  # premultiplied
            for d, colour in layers(x, y, p):
                a = max(0.0, min(1.0, 0.5 - d * n))  # one pixel of anti-aliasing
                if a:
                    acc = [c * a + o * (1 - a) for c, o in zip(colour, acc)]
                    alpha = a + alpha * (1 - a)
            rgb = [round(c / alpha) if alpha else 0 for c in acc]
            row.append((*rgb, round(255 * alpha)))
        rows.append(row)
    return rows


def png(n, rows):
    raw = b"".join(b"\0" + bytes(c for px in row for c in px) for row in rows)
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))  # noqa: E731
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def bmp(n, rows):
    header = struct.pack("<IiiHHIIiiII", 40, n, 2 * n, 1, 32, 0, 0, 0, 0, 0, 0)
    xor = b"".join(bytes((b, g, r, a)) for row in reversed(rows) for r, g, b, a in row)
    mask = b"\0" * (((n + 31) // 32) * 4 * n)  # all opaque: the alpha channel decides
    return header + xor + mask


def main(dst):
    images = [(n, png(n, pixels(n)) if n == 256 else bmp(n, pixels(n))) for n in SIZES]
    out = bytearray(struct.pack("<HHH", 0, 1, len(images)))
    offset = 6 + 16 * len(images)
    for n, data in images:
        out += struct.pack("<BBBBHHII", n % 256, n % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _, data in images:
        out += data
    with open(dst, "wb") as f:
        f.write(out)
    print(f"{dst}: sizes {SIZES}, {len(out)} bytes")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "crates/app/app.ico")
