# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pillow>=12.3.0",
# ]
# ///
"""生成应用图标：同一份几何定义输出深色、浅色两套 SVG / PNG，以及 ICO、ICNS。

用法（在仓库根目录）：

    uv run scripts/generate_app_icons.py

蜘蛛徽标由下面的坐标直接定义，不依赖任何外部图片；修改几何或配色后重新运行本脚本，
并提交 assets/icons/ 下的全部产物。小尺寸使用加粗的腿，否则缩到 16～48 像素时会糊成一团。
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

CANVAS = 1024
SUPERSAMPLE = 4
OUTPUT = Path(__file__).resolve().parent.parent / "assets" / "icons"

Point = tuple[float, float]


@dataclass(frozen=True)
class Palette:
    name: str
    tile_top: str
    tile_bottom: str
    spider_top: str
    spider_bottom: str
    web: tuple[int, int, int, int]


PALETTES = (
    Palette("dark", "#262A33", "#0B0C0F", "#F0443F", "#B3171B", (255, 255, 255, 20)),
    Palette("light", "#FFFFFF", "#E7E1D6", "#E8383A", "#AC1519", (60, 40, 30, 26)),
)

# 右半边四条腿：髋 → 膝 → 足尖，左半边镜像得到。上面两条向上收拢，下面两条向下收拢。
LEGS: tuple[tuple[Point, Point, Point], ...] = (
    ((545, 348), (642, 250), (598, 116)),
    ((560, 378), (762, 300), (774, 138)),
    ((562, 412), (772, 472), (802, 764)),
    ((548, 442), (668, 572), (640, 904)),
)
LEG_HALF_WIDTHS = (17.0, 13.0, 1.5)


def quadratic(start: Point, control: Point, end: Point, steps: int = 24) -> list[Point]:
    points = []
    for index in range(steps + 1):
        t = index / steps
        a, b, c = (1 - t) ** 2, 2 * (1 - t) * t, t**2
        points.append(
            (
                a * start[0] + b * control[0] + c * end[0],
                a * start[1] + b * control[1] + c * end[1],
            )
        )
    return points


def mirrored(points: list[Point]) -> list[Point]:
    return [(CANVAS - x, y) for x, y in points]


def body_outline(top: float, widest: float, bottom: float, half_width: float) -> list[Point]:
    """上圆下尖的轮廓：上半段接近椭圆，下半段收成尖角。"""
    right = quadratic((512, top), (512 + half_width * 1.25, top), (512 + half_width, widest))
    right += quadratic(
        (512 + half_width, widest),
        (512 + half_width * 0.95, (widest + bottom) / 2),
        (512, bottom),
    )[1:]
    return right + mirrored(right)[::-1]


def tapered(points: tuple[Point, ...], half_widths: tuple[float, ...]) -> list[Point]:
    """把折线扩成逐渐变细的多边形，拐点按斜接补偿宽度。"""
    left: list[Point] = []
    right: list[Point] = []
    for index, (point, half) in enumerate(zip(points, half_widths)):
        before = points[max(index - 1, 0)]
        after = points[min(index + 1, len(points) - 1)]
        incoming = _unit((point[0] - before[0], point[1] - before[1])) if index else None
        outgoing = (
            _unit((after[0] - point[0], after[1] - point[1])) if index < len(points) - 1 else None
        )
        if incoming and outgoing:
            direction = _unit((incoming[0] + outgoing[0], incoming[1] + outgoing[1]))
            half /= max(direction[0] * incoming[0] + direction[1] * incoming[1], 0.4)
        else:
            direction = incoming or outgoing or (1.0, 0.0)
        normal = (-direction[1], direction[0])
        left.append((point[0] + normal[0] * half, point[1] + normal[1] * half))
        right.append((point[0] - normal[0] * half, point[1] - normal[1] * half))
    return left + right[::-1]


def _unit(vector: Point) -> Point:
    length = math.hypot(*vector) or 1.0
    return (vector[0] / length, vector[1] / length)


def spider_polygons(bold: float) -> list[list[Point]]:
    half_widths = tuple(width * bold for width in LEG_HALF_WIDTHS)
    polygons = []
    for leg in LEGS:
        polygon = tapered(leg, half_widths)
        polygons += [polygon, mirrored(polygon)]
    polygons.append(body_outline(296, 384, 462, 60 * (1 + (bold - 1) * 0.35)))
    polygons.append(body_outline(438, 566, 812, 94 * (1 + (bold - 1) * 0.35)))
    return polygons


def web_lines() -> list[list[Point]]:
    """以蜘蛛为中心的放射线，加上向内下垂的环线。"""
    center = (512.0, 470.0)
    spokes = 14
    angles = [math.tau * index / spokes + math.pi / spokes for index in range(spokes)]
    lines = [[center, _polar(center, angle, 900)] for angle in angles]
    for radius in (150, 270, 400, 545, 700):
        for index, angle in enumerate(angles):
            following = angles[(index + 1) % spokes]
            if following < angle:
                following += math.tau
            lines.append(
                quadratic(
                    _polar(center, angle, radius),
                    _polar(center, (angle + following) / 2, radius * 0.86),
                    _polar(center, following, radius),
                    steps=10,
                )
            )
    return lines


def _polar(center: Point, angle: float, radius: float) -> Point:
    return (center[0] + math.cos(angle) * radius, center[1] + math.sin(angle) * radius)


def tile_box(inset: float) -> tuple[float, float, float, float, float]:
    """返回底板的左上右下与圆角；macOS 图标按系统网格四周留白。"""
    size = CANVAS - inset * 2
    return (inset, inset, CANVAS - inset, CANVAS - inset, size * 0.225)


def gradient(top: str, bottom: str, size: int) -> Image.Image:
    start, end = (_rgb(color) for color in (top, bottom))
    column = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / max(size - 1, 1)
        column.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(start, end)))
    return column.resize((size, size)).convert("RGBA")


def _rgb(color: str) -> tuple[int, int, int]:
    return tuple(int(color[index : index + 2], 16) for index in (1, 3, 5))


def render(palette: Palette, size: int, *, inset: float, bold: float, web: bool) -> Image.Image:
    scale = size * SUPERSAMPLE / CANVAS
    pixels = size * SUPERSAMPLE

    def scaled(points: list[Point]) -> list[Point]:
        return [(x * scale, y * scale) for x, y in points]

    left, top, right, bottom, radius = tile_box(inset)
    tile_mask = Image.new("L", (pixels, pixels), 0)
    ImageDraw.Draw(tile_mask).rounded_rectangle(
        (left * scale, top * scale, right * scale, bottom * scale), radius * scale, fill=255
    )
    image = Image.new("RGBA", (pixels, pixels), (0, 0, 0, 0))
    image.paste(gradient(palette.tile_top, palette.tile_bottom, pixels), (0, 0), tile_mask)

    if web:
        layer = Image.new("RGBA", (pixels, pixels), (0, 0, 0, 0))
        draw = ImageDraw.Draw(layer)
        for line in web_lines():
            draw.line(scaled(line), fill=palette.web, width=max(round(5 * scale), 1))
        layer.putalpha(ImageChops.multiply(layer.getchannel("A"), tile_mask))
        image = Image.alpha_composite(image, layer)

    # 蜘蛛按底板大小等比缩放并居中，macOS 留白后不会顶到底板边缘。
    shrink = (right - left) / CANVAS
    spider_mask = Image.new("L", (pixels, pixels), 0)
    draw = ImageDraw.Draw(spider_mask)
    for polygon in spider_polygons(bold):
        placed = [(left + x * shrink, top + y * shrink) for x, y in polygon]
        draw.polygon(scaled(placed), fill=255)
    image.paste(gradient(palette.spider_top, palette.spider_bottom, pixels), (0, 0), spider_mask)
    return image.resize((size, size), Image.Resampling.LANCZOS)


def for_size(palette: Palette, size: int, *, inset: float = 0.0) -> Image.Image:
    """小尺寸加粗并去掉蛛网，避免细线在缩小后变成噪点。"""
    bold = 2.0 if size <= 24 else 1.6 if size <= 48 else 1.25 if size <= 128 else 1.0
    return render(palette, size, inset=inset, bold=bold, web=size > 64)


def svg(palette: Palette) -> str:
    left, top, right, bottom, radius = tile_box(0.0)
    paths = "\n".join(
        '    <path d="M{} Z"/>'.format(" L".join(f"{x:.1f},{y:.1f}" for x, y in polygon))
        for polygon in spider_polygons(1.0)
    )
    web = "\n".join(
        '    <polyline points="{}"/>'.format(" ".join(f"{x:.1f},{y:.1f}" for x, y in line))
        for line in web_lines()
    )
    red, green, blue, alpha = palette.web
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<!-- 由 scripts/generate_app_icons.py 生成，请修改脚本后重新生成，不要手工编辑。 -->
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {CANVAS} {CANVAS}">
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{palette.tile_top}"/>
      <stop offset="1" stop-color="{palette.tile_bottom}"/>
    </linearGradient>
    <linearGradient id="spider" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="{CANVAS}">
      <stop offset="0" stop-color="{palette.spider_top}"/>
      <stop offset="1" stop-color="{palette.spider_bottom}"/>
    </linearGradient>
    <clipPath id="clip">
      <rect x="{left}" y="{top}" width="{right - left}" height="{bottom - top}" rx="{radius:.1f}"/>
    </clipPath>
  </defs>
  <rect x="{left}" y="{top}" width="{right - left}" height="{bottom - top}" rx="{radius:.1f}" fill="url(#tile)"/>
  <g clip-path="url(#clip)" fill="none" stroke="rgb({red},{green},{blue})" stroke-opacity="{alpha / 255:.3f}" stroke-width="5">
{web}
  </g>
  <g fill="url(#spider)">
{paths}
  </g>
</svg>
"""


def main() -> None:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for palette in PALETTES:
        (OUTPUT / f"app-icon-{palette.name}.svg").write_text(svg(palette), encoding="utf-8")
        # 运行时随主题切换：窗口小图标、任务栏大图标，以及按 macOS 网格留白的 Dock 图标。
        for_size(palette, 32).save(OUTPUT / f"window-{palette.name}-32.png", optimize=True)
        for_size(palette, 256).save(OUTPUT / f"window-{palette.name}-256.png", optimize=True)
        for_size(palette, 512, inset=100.0).save(OUTPUT / f"dock-{palette.name}.png", optimize=True)

    # 安装包里的静态图标不能随主题变化，统一使用默认主题（石墨青）对应的深色版。
    dark = PALETTES[0]
    ico_sizes = (256, 128, 64, 48, 32, 24, 16)
    ico = [for_size(dark, size) for size in ico_sizes]
    ico[0].save(
        OUTPUT / "app-icon.ico",
        sizes=[(size, size) for size in ico_sizes],
        append_images=ico[1:],
    )
    icns_sizes = (1024, 512, 256, 128, 64, 32, 16)
    icns = [for_size(dark, size, inset=100.0) for size in icns_sizes]
    icns[0].save(OUTPUT / "app-icon.icns", append_images=icns[1:])
    for_size(dark, 512).save(OUTPUT / "app-icon.png", optimize=True)
    for path in sorted(OUTPUT.iterdir()):
        print(f"{path.stat().st_size:>8}  {path.relative_to(OUTPUT.parent.parent)}")


if __name__ == "__main__":
    main()
