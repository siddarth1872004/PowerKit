#!/usr/bin/env python3
"""Generate PowerKit's icon assets (original artwork, drawn procedurally).

Outputs assets/icon.ico plus the PNG sizes Tauri's bundler expects.
Pure standard library: no Pillow required.
"""
import os, struct, zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SS = 4  # supersampling factor


def rounded(px, py, x0, y0, x1, y1, r):
    """Is the point inside the rounded rectangle?"""
    if px < x0 or px > x1 or py < y0 or py > y1:
        return False
    cx = min(max(px, x0 + r), x1 - r)
    cy = min(max(py, y0 + r), y1 - r)
    return (px - cx) ** 2 + (py - cy) ** 2 <= r * r


def sample(u, v):
    """Color (r, g, b, a) at unit coordinates."""
    if not rounded(u, v, 0.04, 0.04, 0.96, 0.96, 0.22):
        return (0, 0, 0, 0)
    t = (u + v) / 2  # diagonal gradient: azure -> violet
    bg = (int(28 + t * 80), int(120 - t * 50), int(255 - t * 15))
    # Three tiles + one accent tile suggest a kit of tools / window zones.
    tiles = [
        (0.20, 0.20, 0.47, 0.47, (255, 255, 255)),
        (0.53, 0.20, 0.80, 0.47, (255, 255, 255)),
        (0.20, 0.53, 0.47, 0.80, (255, 255, 255)),
        (0.53, 0.53, 0.80, 0.80, (255, 214, 92)),
    ]
    for x0, y0, x1, y1, c in tiles:
        if rounded(u, v, x0, y0, x1, y1, 0.065):
            return (*c, 255)
    return (*bg, 255)


def render(size):
    out = bytearray()
    n = SS * SS
    for y in range(size):
        for x in range(size):
            r = g = b = a = 0
            for sy in range(SS):
                for sx in range(SS):
                    cr, cg, cb, ca = sample((x + (sx + 0.5) / SS) / size, (y + (sy + 0.5) / SS) / size)
                    r += cr * ca; g += cg * ca; b += cb * ca; a += ca
            if a:
                out += bytes((r // a, g // a, b // a, a // n))
            else:
                out += b"\0\0\0\0"
    return bytes(out)


def png(size, rgba):
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
    raw = b"".join(b"\0" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def bmp_entry(size, rgba):
    """32-bit BGRA DIB (bottom-up) followed by an all-zero AND mask, as stored in .ico."""
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    rows = []
    for y in range(size - 1, -1, -1):
        row = rgba[y * size * 4:(y + 1) * size * 4]
        rows.append(b"".join(bytes((row[i + 2], row[i + 1], row[i], row[i + 3])) for i in range(0, len(row), 4)))
    mask_row = b"\0" * (((size + 31) // 32) * 4)
    return header + b"".join(rows) + mask_row * size


def main():
    images = {s: render(s) for s in (16, 24, 32, 48, 64, 128, 256)}
    entries = [(s, bmp_entry(s, images[s])) for s in (16, 24, 32, 48, 64)] + [(256, png(256, images[256]))]
    ico = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    for s, data in entries:
        ico += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    ico += b"".join(d for _, d in entries)
    os.makedirs(os.path.join(ROOT, "assets"), exist_ok=True)
    with open(os.path.join(ROOT, "assets", "icon.ico"), "wb") as f:
        f.write(ico)
    icons = os.path.join(ROOT, "apps", "desktop", "src-tauri", "icons")
    os.makedirs(icons, exist_ok=True)
    with open(os.path.join(icons, "icon.ico"), "wb") as f:
        f.write(ico)
    for name, s in (("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 256)):
        with open(os.path.join(icons, name), "wb") as f:
            f.write(png(s, images[s]))
    print("icons written")


if __name__ == "__main__":
    main()
