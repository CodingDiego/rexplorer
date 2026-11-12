"""Draws rexplorer's own icon (a white "r" on a rounded teal-to-blue square) into crates/app/app.ico.

    python -I scripts/make-icon.py [dest.ico]

Standard library only. Shapes are signed distance fields, so every size is drawn directly with smooth edges.
Sizes 16-64 are 32-bit BMP entries, 256 is PNG, as Windows expects.
"""
import math
import struct
import sys
import zlib

SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
TOP, BOTTOM = (0x14, 0xA3, 0x9B), (0x25, 0x4E, 0xC8)
STROKE = 0.115  # glyph stroke width, as a fraction of the icon


def rounded_square(x, y):
    half, radius = 0.44, 0.12
    qx, qy = abs(x - 0.5) - half + radius, abs(y - 0.5) - half + radius
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - radius


def segment(x, y, ax, ay, bx, by):
    """Distance to the segment a-b, minus half the stroke (a capsule)."""
    px, py, dx, dy = x - ax, y - ay, bx - ax, by - ay
    t = max(0.0, min(1.0, (px * dx + py * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - t * dx, py - t * dy) - STROKE / 2


def shoulder(x, y):
    """The r's arc: from the stem's top, over and down to the right, with round ends."""
    cx, cy, r = 0.51, 0.485, 0.165
    a0, a1 = math.radians(20), math.radians(180)  # counterclockwise, y up
    ang = math.atan2(cy - y, x - cx)
    if a0 <= ang <= a1:
        return abs(math.hypot(x - cx, y - cy) - r) - STROKE / 2
    ends = [(cx + r * math.cos(a), cy - r * math.sin(a)) for a in (a0, a1)]
    return min(math.hypot(x - ex, y - ey) for ex, ey in ends) - STROKE / 2


def glyph(x, y):
    return min(segment(x, y, 0.345, 0.31, 0.345, 0.70), shoulder(x, y))


def pixels(n):
    """Rows of (r, g, b, a), top to bottom."""
    rows = []
    for j in range(n):
        row = []
        for i in range(n):
            x, y = (i + 0.5) / n, (j + 0.5) / n
            cover = lambda d: max(0.0, min(1.0, 0.5 - d * n))  # noqa: E731 - one pixel of anti-aliasing
            bg, fg = cover(rounded_square(x, y)), cover(glyph(x, y))
            t = y
            base = [TOP[k] + (BOTTOM[k] - TOP[k]) * t for k in range(3)]
            rgb = [round(c + (255 - c) * fg) for c in base]
            row.append((*rgb, round(255 * bg)))
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
