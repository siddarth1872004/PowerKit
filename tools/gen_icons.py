#!/usr/bin/env python3
"""Generate PowerKit's icon assets (original artwork, drawn procedurally).

Outputs assets/icon.ico (the app) and assets/modules/<id>.ico and .png (one per
module, for the settings window, module windows and the README).
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


# Module icons: a white glyph (with one yellow accent) on a rounded square whose
# gradient gives each module its own colour. Shapes are in unit coordinates and
# painted in order; None as a colour means "the background shows through".

W = (255, 255, 255, 255)
Y = (255, 214, 92, 255)


def seg_dist(px, py, x0, y0, x1, y1):
    dx, dy = x1 - x0, y1 - y0
    t = 0.0 if dx == dy == 0 else max(0.0, min(1.0, ((px - x0) * dx + (py - y0) * dy) / (dx * dx + dy * dy)))
    return ((px - x0 - t * dx) ** 2 + (py - y0 - t * dy) ** 2) ** 0.5


def in_poly(px, py, pts):
    inside = False
    j = len(pts) - 1
    for i in range(len(pts)):
        (xi, yi), (xj, yj) = pts[i], pts[j]
        if (yi > py) != (yj > py) and px < (xj - xi) * (py - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside


def rrect(x0, y0, x1, y1, r=0.05):
    return lambda u, v: rounded(u, v, x0, y0, x1, y1, r)


def frame(x0, y0, x1, y1, r, w):
    return lambda u, v: rounded(u, v, x0, y0, x1, y1, r) and not rounded(u, v, x0 + w, y0 + w, x1 - w, y1 - w, max(r - w, 0))


def circle(cx, cy, r):
    return lambda u, v: (u - cx) ** 2 + (v - cy) ** 2 <= r * r


def ring(cx, cy, r, w):
    return lambda u, v: abs(((u - cx) ** 2 + (v - cy) ** 2) ** 0.5 - r) <= w / 2


def ering(cx, cy, rx, ry, w):
    return lambda u, v: abs((((u - cx) / rx) ** 2 + ((v - cy) / ry) ** 2) ** 0.5 - 1) * min(rx, ry) <= w / 2


def line(x0, y0, x1, y1, w=0.07):
    return lambda u, v: seg_dist(u, v, x0, y0, x1, y1) <= w / 2


def lines(pts, w=0.07):
    return lambda u, v: any(seg_dist(u, v, *pts[i], *pts[i + 1]) <= w / 2 for i in range(len(pts) - 1))


def poly(pts):
    return lambda u, v: in_poly(u, v, pts)


def clip(shape, test):
    return lambda u, v: test(u, v) and shape(u, v)


def fade(color, a):
    return (*color[:3], int(255 * a))


MODULES = {
    "power_rename": ((52, 120, 246), (99, 73, 227), [
        (W, frame(0.14, 0.34, 0.86, 0.66, 0.08, 0.07)),
        (W, rrect(0.25, 0.46, 0.5, 0.54, 0.02)),
        (Y, line(0.64, 0.22, 0.64, 0.78)), (Y, line(0.56, 0.22, 0.72, 0.22, 0.06)), (Y, line(0.56, 0.78, 0.72, 0.78, 0.06)),
    ]),
    "always_on_top": ((233, 84, 96), (176, 48, 120), [
        (W, rrect(0.36, 0.18, 0.64, 0.32, 0.04)),
        (W, poly([(0.41, 0.32), (0.59, 0.32), (0.66, 0.56), (0.34, 0.56)])),
        (W, line(0.24, 0.57, 0.76, 0.57, 0.08)),
        (Y, line(0.5, 0.6, 0.5, 0.84, 0.07)),
    ]),
    "color_picker": ((255, 138, 61), (222, 62, 112), [
        (W, line(0.3, 0.7, 0.62, 0.38, 0.11)),
        (W, circle(0.69, 0.31, 0.13)),
        (W, line(0.52, 0.34, 0.66, 0.48, 0.08)),
        (Y, line(0.21, 0.79, 0.32, 0.68, 0.11)),
    ]),
    "launcher": ((35, 166, 213), (48, 92, 214), [
        (W, ring(0.43, 0.43, 0.2, 0.09)),
        (Y, line(0.58, 0.58, 0.79, 0.79, 0.12)),
    ]),
    "fancy_zones": ((49, 178, 128), (22, 120, 140), [
        (W, rrect(0.16, 0.18, 0.46, 0.82, 0.05)),
        (Y, rrect(0.52, 0.18, 0.84, 0.47, 0.05)),
        (W, rrect(0.52, 0.53, 0.84, 0.82, 0.05)),
    ]),
    "keyboard_manager": ((116, 124, 146), (58, 66, 90), [
        (W, frame(0.12, 0.28, 0.88, 0.74, 0.08, 0.06)),
        *[(W, rrect(x, 0.39, x + 0.09, 0.48, 0.02)) for x in (0.22, 0.355, 0.49, 0.625)],
        (Y, rrect(0.3, 0.56, 0.7, 0.63, 0.03)),
    ]),
    "awake": ((182, 116, 66), (112, 66, 40), [
        (W, rrect(0.22, 0.4, 0.62, 0.8, 0.08)),
        (W, clip(ring(0.64, 0.58, 0.11, 0.07), lambda u, v: u > 0.6)),
        (Y, line(0.34, 0.16, 0.34, 0.3, 0.06)), (Y, line(0.5, 0.16, 0.5, 0.3, 0.06)),
    ]),
    "light_switch": ((250, 172, 40), (76, 66, 176), [
        (Y, clip(circle(0.5, 0.5, 0.27), lambda u, v: u <= 0.5)),
        (W, ring(0.5, 0.5, 0.27, 0.07)),
        (W, line(0.5, 0.23, 0.5, 0.77, 0.05)),
    ]),
    "quick_accent": ((222, 80, 170), (130, 58, 200), [
        (W, rrect(0.12, 0.46, 0.88, 0.76, 0.08)),
        (Y, rrect(0.39, 0.5, 0.61, 0.72, 0.05)),
        (W, line(0.45, 0.38, 0.57, 0.22, 0.08)),
    ]),
    "mouse_utilities": ((52, 196, 200), (38, 116, 200), [
        (W, frame(0.3, 0.16, 0.7, 0.84, 0.2, 0.07)),
        (W, line(0.5, 0.17, 0.5, 0.44, 0.05)),
        (Y, rrect(0.46, 0.27, 0.54, 0.39, 0.03)),
    ]),
    "crop_and_lock": ((118, 186, 60), (36, 136, 90), [
        (W, lines([(0.3, 0.14), (0.3, 0.7), (0.86, 0.7)], 0.09)),
        (Y, lines([(0.14, 0.3), (0.7, 0.3), (0.7, 0.86)], 0.09)),
    ]),
    "shortcut_guide": ((92, 104, 240), (48, 56, 166), [
        *[(Y if (x, y) == (0.4, 0.29) else W, rrect(x, y, x + 0.2, y + 0.19, 0.04))
          for x in (0.17, 0.4, 0.63) for y in (0.29, 0.52)],
    ]),
    "grab_and_move": ((240, 112, 70), (196, 56, 64), [
        (W, line(0.5, 0.22, 0.5, 0.78, 0.07)), (W, line(0.22, 0.5, 0.78, 0.5, 0.07)),
        (W, poly([(0.5, 0.12), (0.39, 0.25), (0.61, 0.25)])), (W, poly([(0.5, 0.88), (0.39, 0.75), (0.61, 0.75)])),
        (W, poly([(0.12, 0.5), (0.25, 0.39), (0.25, 0.61)])), (W, poly([(0.88, 0.5), (0.75, 0.39), (0.75, 0.61)])),
        (Y, circle(0.5, 0.5, 0.08)),
    ]),
    "window_hopper": ((98, 170, 250), (66, 84, 218), [
        (fade(W, 0.45), rrect(0.4, 0.16, 0.86, 0.5, 0.05)),
        (fade(W, 0.7), rrect(0.27, 0.31, 0.73, 0.65, 0.05)),
        (W, rrect(0.14, 0.46, 0.6, 0.84, 0.05)),
        (Y, rrect(0.14, 0.46, 0.6, 0.55, 0.05)),
    ]),
    "hosts_editor": ((40, 152, 122), (28, 86, 132), [
        (W, ring(0.5, 0.5, 0.3, 0.065)),
        (W, ering(0.5, 0.5, 0.13, 0.3, 0.06)),
        (W, line(0.22, 0.5, 0.78, 0.5, 0.06)),
        (Y, line(0.29, 0.36, 0.71, 0.36, 0.05)), (Y, line(0.29, 0.64, 0.71, 0.64, 0.05)),
    ]),
    "environment_variables": ((100, 188, 88), (36, 126, 80), [
        (W, lines([(0.38, 0.18), (0.29, 0.23), (0.29, 0.44), (0.2, 0.5), (0.29, 0.56), (0.29, 0.77), (0.38, 0.82)], 0.065)),
        (W, lines([(0.62, 0.18), (0.71, 0.23), (0.71, 0.44), (0.8, 0.5), (0.71, 0.56), (0.71, 0.77), (0.62, 0.82)], 0.065)),
        (Y, line(0.42, 0.41, 0.58, 0.59, 0.075)), (Y, line(0.58, 0.41, 0.42, 0.59, 0.075)),
    ]),
    "registry_preview": ((92, 122, 172), (48, 66, 118), [
        (W, rrect(0.16, 0.16, 0.4, 0.34, 0.04)),
        (W, line(0.28, 0.34, 0.28, 0.72, 0.055)),
        (W, line(0.28, 0.5, 0.48, 0.5, 0.055)), (W, line(0.28, 0.72, 0.48, 0.72, 0.055)),
        (Y, rrect(0.46, 0.41, 0.84, 0.59, 0.04)),
        (W, rrect(0.46, 0.63, 0.84, 0.81, 0.04)),
    ]),
    "new_plus": ((60, 170, 255), (38, 104, 228), [
        (W, poly([(0.22, 0.14), (0.56, 0.14), (0.74, 0.32), (0.74, 0.86), (0.22, 0.86)])),
        (fade((40, 90, 180), 0.35), poly([(0.56, 0.14), (0.56, 0.32), (0.74, 0.32)])),
        (None, circle(0.7, 0.72, 0.19)),
        (Y, line(0.7, 0.6, 0.7, 0.84, 0.08)), (Y, line(0.58, 0.72, 0.82, 0.72, 0.08)),
    ]),
    "file_locksmith": ((255, 168, 64), (214, 84, 52), [
        (W, lines([(0.35, 0.47), (0.35, 0.33), (0.41, 0.21), (0.59, 0.21), (0.65, 0.33), (0.65, 0.37)], 0.075)),
        (W, rrect(0.24, 0.46, 0.76, 0.85, 0.07)),
        (Y, circle(0.5, 0.6, 0.065)), (Y, line(0.5, 0.6, 0.5, 0.74, 0.055)),
    ]),
    "image_resizer": ((255, 122, 150), (172, 64, 196), [
        (fade(W, 0.55), frame(0.14, 0.14, 0.86, 0.86, 0.06, 0.05)),
        (W, rrect(0.14, 0.44, 0.56, 0.86, 0.05)),
        (fade((150, 52, 150), 0.7), poly([(0.18, 0.82), (0.31, 0.62), (0.41, 0.74), (0.46, 0.68), (0.53, 0.82)])),
        (Y, line(0.52, 0.48, 0.72, 0.28, 0.07)),
        (Y, poly([(0.84, 0.16), (0.62, 0.2), (0.8, 0.38)])),
    ]),
    "text_extractor": ((96, 214, 196), (32, 120, 168), [
        (fade(W, 0.55), frame(0.12, 0.12, 0.88, 0.88, 0.05, 0.05)),
        (W, line(0.26, 0.34, 0.74, 0.34, 0.07)),
        (W, line(0.26, 0.5, 0.66, 0.5, 0.07)),
        (W, line(0.26, 0.66, 0.56, 0.66, 0.07)),
        (Y, line(0.64, 0.66, 0.84, 0.86, 0.08)),
    ]),
    "power_display": ((112, 160, 255), (52, 64, 168), [
        (W, frame(0.12, 0.14, 0.88, 0.68, 0.06, 0.06)),
        (W, line(0.5, 0.68, 0.5, 0.8, 0.07)),
        (W, line(0.32, 0.84, 0.68, 0.84, 0.07)),
        (Y, circle(0.5, 0.41, 0.08)),
        (Y, line(0.5, 0.22, 0.5, 0.27, 0.045)), (Y, line(0.5, 0.55, 0.5, 0.6, 0.045)),
        (Y, line(0.31, 0.41, 0.36, 0.41, 0.045)), (Y, line(0.64, 0.41, 0.69, 0.41, 0.045)),
    ]),
    "zoom_it": ((120, 200, 255), (40, 96, 200), [
        (W, ring(0.42, 0.42, 0.24, 0.07)),
        (W, line(0.6, 0.6, 0.82, 0.82, 0.1)),
        (Y, line(0.32, 0.42, 0.52, 0.42, 0.06)),
        (Y, line(0.42, 0.32, 0.42, 0.52, 0.06)),
    ]),
    "screen_ruler": ((255, 170, 96), (200, 72, 40), [
        (W, rrect(0.12, 0.56, 0.88, 0.84, 0.04)),
        (fade((200, 72, 40), 0.8), line(0.26, 0.56, 0.26, 0.68, 0.04)),
        (fade((200, 72, 40), 0.8), line(0.4, 0.56, 0.4, 0.64, 0.04)),
        (fade((200, 72, 40), 0.8), line(0.54, 0.56, 0.54, 0.68, 0.04)),
        (fade((200, 72, 40), 0.8), line(0.68, 0.56, 0.68, 0.64, 0.04)),
        (Y, line(0.18, 0.32, 0.82, 0.32, 0.06)),
        (Y, line(0.18, 0.2, 0.18, 0.44, 0.06)), (Y, line(0.82, 0.2, 0.82, 0.44, 0.06)),
    ]),
    "workspaces": ((140, 220, 200), (24, 120, 120), [
        (W, rrect(0.1, 0.16, 0.56, 0.54, 0.05)),
        (W, rrect(0.62, 0.16, 0.9, 0.54, 0.05)),
        (W, rrect(0.1, 0.6, 0.9, 0.86, 0.05)),
        (Y, line(0.1, 0.24, 0.56, 0.24, 0.05)),
        (Y, line(0.62, 0.24, 0.9, 0.24, 0.05)),
        (Y, line(0.1, 0.68, 0.9, 0.68, 0.05)),
    ]),
    "power_plan": ((250, 210, 110), (200, 110, 20), [
        (W, poly([(0.56, 0.1), (0.26, 0.56), (0.48, 0.56), (0.42, 0.9), (0.74, 0.42), (0.52, 0.42)])),
    ]),
    "advanced_paste": ((120, 200, 250), (40, 96, 200), [
        (W, rrect(0.2, 0.18, 0.8, 0.88, 0.07)),
        (fade(W, 0.55), rrect(0.36, 0.1, 0.64, 0.26, 0.05)),
        (Y, line(0.32, 0.46, 0.68, 0.46, 0.07)),
        (Y, line(0.32, 0.62, 0.6, 0.62, 0.07)),
        (Y, line(0.32, 0.78, 0.52, 0.78, 0.07)),
    ]),
    "command_not_found": ((150, 150, 165), (50, 50, 70), [
        (W, frame(0.12, 0.18, 0.88, 0.82, 0.06, 0.06)),
        (Y, lines([(0.26, 0.38), (0.4, 0.5), (0.26, 0.62)], 0.07)),
        (W, ring(0.64, 0.6, 0.09, 0.05)),
        (W, line(0.71, 0.68, 0.8, 0.77, 0.06)),
    ]),
    "peek": ((150, 190, 250), (40, 80, 180), [
        (W, ering(0.5, 0.5, 0.38, 0.22, 0.06)),
        (W, circle(0.5, 0.5, 0.13)),
        (Y, circle(0.5, 0.5, 0.05)),
    ]),
}


def module_sampler(top, bottom, shapes):
    def sample_at(u, v):
        if not rounded(u, v, 0.04, 0.04, 0.96, 0.96, 0.22):
            return (0, 0, 0, 0)
        t = (u + v) / 2
        bg = tuple(int(a + (b - a) * t) for a, b in zip(top, bottom))
        c = bg
        for color, inside in shapes:
            if inside(u, v):
                if color is None:
                    c = bg
                else:
                    a = color[3] / 255
                    c = tuple(int(x * a + y * (1 - a)) for x, y in zip(color[:3], c))
        return (*c, 255)
    return sample_at


def ico(images, png_sizes=()):
    """An .ico holding `images` ({size: rgba}); sizes in `png_sizes` stored as PNG."""
    entries = [(s, png(s, images[s]) if s in png_sizes else bmp_entry(s, images[s])) for s in sorted(images)]
    out = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    for s, data in entries:
        out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return out + b"".join(d for _, d in entries)


def main():
    global sample
    images = {s: render(s) for s in (16, 24, 32, 48, 64, 256)}
    with open(os.path.join(ROOT, "assets", "icon.ico"), "wb") as f:
        f.write(ico(images, png_sizes=(256,)))
    app_sample = sample
    out = os.path.join(ROOT, "assets", "modules")
    os.makedirs(out, exist_ok=True)
    for name, (top, bottom, shapes) in MODULES.items():
        sample = module_sampler(top, bottom, shapes)
        images = {s: render(s) for s in (16, 20, 24, 32, 40, 48, 64)}
        with open(os.path.join(out, name + ".ico"), "wb") as f:
            f.write(ico(images))
        with open(os.path.join(out, name + ".png"), "wb") as f:
            f.write(png(64, images[64]))
    sample = app_sample
    print(f"app icon and {len(MODULES)} module icons written")


if __name__ == "__main__":
    main()
