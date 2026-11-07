"""Renders installer/AerOS.ico: the leaf logo from kernel/src/logo.rs, white on
a dark teal rounded square. Usage: python -I tools/make-installer-icon.py"""
import math
import pathlib
import re
import sys

from PIL import Image, ImageDraw

root = pathlib.Path(__file__).resolve().parent.parent
source = (root / "kernel" / "src" / "logo.rs").read_text(encoding="utf-8")


def points(name):
    body = re.search(r"const " + name + r": \[[^=]*= \[(.*?)\];", source, re.S).group(1)
    return [tuple(map(int, pair)) for pair in re.findall(r"\((-?\d+), (-?\d+)\)", body)]


def constant(name):
    return int(re.search(r"const " + name + r": (?:i32|usize) = (\d+);", source).group(1))


leaf = points("LEAF")
stem = points("STEM")
veins = [tuple(map(int, v)) for v in re.findall(
    r"\(\((\d+), (\d+)\), \((\d+), (\d+)\), (\d+)\)", re.search(r"const VEINS.*?\];", source, re.S).group(0))]
outline_points = constant("STEM_OUTLINE_POINTS")
left, top = constant("SOURCE_LEFT"), constant("SOURCE_TOP")
width, height = constant("SOURCE_WIDTH"), constant("SOURCE_HEIGHT")

scale = 4
mask = Image.new("L", (width * scale, height * scale), 0)
draw = ImageDraw.Draw(mask)


def shifted(seq):
    return [((x - left) * scale, (y - top) * scale) for x, y in seq]


def stroke(seq, line_width, fill):
    seq = shifted(seq)
    draw.line(seq, fill=fill, width=line_width * scale)
    for x, y in seq:
        r = line_width * scale / 2
        draw.ellipse((x - r, y - r, x + r, y + r), fill=fill)


draw.polygon(shifted(leaf), fill=255)
stroke(stem[:outline_points], constant("STEM_OUTLINE_WIDTH"), 255)
stroke(stem, constant("STEM_WIDTH"), 0)
for ax, ay, bx, by, w in veins:
    stroke([(ax, ay), (bx, by)], w, 0)
bbox = mask.getbbox()
mask = mask.crop(bbox)

size = 256
icon = Image.new("RGBA", (size, size), (0, 0, 0, 0))
tile = Image.new("L", (size * 4, size * 4), 0)
ImageDraw.Draw(tile).rounded_rectangle((0, 0, size * 4 - 1, size * 4 - 1), radius=size * 4 * 22 // 100, fill=255)
tile = tile.resize((size, size), Image.LANCZOS)
background = Image.new("RGBA", (size, size), (0, 0, 0, 0))
for y in range(size):
    t = y / (size - 1)
    color = (int(38 + 14 * t), int(96 - 30 * t), int(108 - 28 * t), 255)
    ImageDraw.Draw(background).line([(0, y), (size, y)], fill=color)
icon.paste(background, (0, 0), tile)
fit = int(size * 0.68)
ratio = min(fit / mask.width, fit / mask.height)
logo = mask.resize((max(1, math.floor(mask.width * ratio)), max(1, math.floor(mask.height * ratio))), Image.LANCZOS)
white = Image.new("RGBA", logo.size, (255, 255, 255, 255))
icon.paste(white, ((size - logo.width) // 2, (size - logo.height) // 2), logo)

out = root / "installer" / "AerOS.ico"
icon.save(out, sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
icon.save(root / "build" / "installer-icon.png")
print(out)
sys.exit(0)
