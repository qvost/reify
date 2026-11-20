"""Regenerate the original Shaligram spiral and antialiased PNG icons (stdlib only)."""
from pathlib import Path
import math
import struct
import zlib

ROOT = Path(__file__).resolve().parent
POINTS = []
for step in range(901):
    angle = step / 900 * math.pi * 4.4
    radius = 4 + 2.85 * angle
    POINTS.append((64 + radius * math.cos(angle - 1.2), 64 + radius * math.sin(angle - 1.2)))
path = "M" + " L".join(f"{x:.3f},{y:.3f}" for x, y in POINTS)
ROOT.joinpath("emblem.svg").write_text(
    f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128">'
    f'<path d="{path}" fill="none" stroke="#141414" stroke-width="5" '
    'stroke-linecap="round" stroke-linejoin="round"/></svg>\n'
)


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


for size in (16, 48, 128):
    scale = 4
    width = size * scale
    pixels = bytearray([240, 238, 232, 255] * width * width)
    stroke = max(5 / 128 * width / 2, scale / 2)
    for x, y in POINTS:
        x, y = x / 128 * width, y / 128 * width
        for py in range(max(0, int(y - stroke)), min(width, math.ceil(y + stroke))):
            for px in range(max(0, int(x - stroke)), min(width, math.ceil(x + stroke))):
                if (px + .5 - x) ** 2 + (py + .5 - y) ** 2 <= stroke ** 2:
                    index = (py * width + px) * 4
                    pixels[index:index + 4] = bytes((20, 20, 20, 255))
    rows = bytearray()
    for y in range(size):
        rows.append(0)
        for x in range(size):
            for channel in range(4):
                rows.append(round(sum(
                    pixels[((y * scale + dy) * width + x * scale + dx) * 4 + channel]
                    for dy in range(scale) for dx in range(scale)
                ) / scale ** 2))
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b"")
    ROOT.joinpath(f"icon-{size}.png").write_bytes(png)
