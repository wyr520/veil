#!/usr/bin/env python3
"""Generate the launcher icon (and a monochrome notification icon) without any
image library: plain RGBA buffers written out as PNG with zlib."""

import struct
import zlib
import os
import math


def write_png(path, width, height, pixels):
    raw = b"".join(b"\x00" + bytes(pixels[y * width * 4:(y + 1) * width * 4]) for y in range(height))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as fh:
        fh.write(png)


def mix(c1, c2, t):
    return tuple(int(c1[i] + (c2[i] - c1[i]) * t) for i in range(3))


def seg_dist(px, py, ax, ay, bx, by):
    vx, vy = bx - ax, by - ay
    wx, wy = px - ax, py - ay
    denom = vx * vx + vy * vy
    t = 0.0 if denom == 0 else max(0.0, min(1.0, (wx * vx + wy * vy) / denom))
    dx, dy = px - (ax + t * vx), py - (ay + t * vy)
    return math.hypot(dx, dy)


def build(size, radius_ratio=0.235, glyph=True):
    px = bytearray(size * size * 4)
    r = int(size * radius_ratio)
    top, bottom = (0x3D, 0xDC, 0x97), (0x2B, 0xB3, 0xFF)
    for y in range(size):
        for x in range(size):
            # rounded rectangle mask with 1px antialias
            dx = max(r - x, x - (size - 1 - r), 0)
            dy = max(r - y, y - (size - 1 - r), 0)
            dist = math.hypot(dx, dy)
            alpha = 255 if (dx == 0 or dy == 0) and dist <= r else (
                255 if dist <= r - 1 else max(0, int(255 * (r - dist))))
            t = (x + y) / (2.0 * (size - 1))
            c = mix(top, bottom, t)
            i = (y * size + x) * 4
            px[i:i + 4] = bytes([c[0], c[1], c[2], alpha])
    if glyph:
        w = max(2.0, size * 0.085)
        ax, ay = size * 0.28, size * 0.30
        bx, by = size * 0.50, size * 0.72
        cx, cy = size * 0.72, size * 0.30
        for y in range(size):
            for x in range(size):
                d = min(seg_dist(x, y, ax, ay, bx, by), seg_dist(x, y, bx, by, cx, cy))
                if d < w / 2 + 1:
                    cov = 1.0 if d < w / 2 else max(0.0, 1.0 - (d - w / 2))
                    i = (y * size + x) * 4
                    a = px[i + 3] / 255.0
                    k = cov * a
                    px[i] = int(px[i] * (1 - k) + 0x06 * k)
                    px[i + 1] = int(px[i + 1] * (1 - k) + 0x12 * k)
                    px[i + 2] = int(px[i + 2] * (1 - k) + 0x1A * k)


    return px


def main():
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    targets = {
        "res/mipmap-mdpi/ic_launcher.png": 48,
        "res/mipmap-hdpi/ic_launcher.png": 72,
        "res/mipmap-xhdpi/ic_launcher.png": 96,
        "res/mipmap-xxhdpi/ic_launcher.png": 144,
        "res/mipmap-xxxhdpi/ic_launcher.png": 192,
    }
    for rel, size in targets.items():
        path = os.path.join(here, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        write_png(path, size, size, build(size))
        print("wrote", rel, size)


if __name__ == "__main__":
    main()
