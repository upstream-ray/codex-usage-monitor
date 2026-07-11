"""Generate deterministic PNG and ICO assets from the Codex Usage icon geometry."""

from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
ICON_DIR = ROOT / "src" / "icons"
GITHUB_DIR = ROOT / ".github"
VERIFY_PATH = ROOT / "target" / "codex-usage-icon-verification.png"

NAVY_TOP = (20, 38, 64, 255)
NAVY_BOTTOM = (7, 15, 28, 255)
CYAN = "#36C5F0"
WHITE = "#F8FAFC"
LIME = "#A3E635"


def render_icon(size: int) -> Image.Image:
    scale = max(4, 1024 // size)
    canvas = size * scale
    image = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    def px(value: float) -> int:
        return round(value * canvas / 256)

    # A restrained vertical gradient keeps the large icon dimensional while
    # preserving a solid silhouette when Windows scales it down to 16 px.
    gradient = Image.new("RGBA", (canvas, canvas))
    pixels = gradient.load()
    for y in range(canvas):
        t = y / max(1, canvas - 1)
        color = tuple(round(a + (b - a) * t) for a, b in zip(NAVY_TOP, NAVY_BOTTOM))
        for x in range(canvas):
            pixels[x, y] = color
    mask = Image.new("L", (canvas, canvas), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, canvas - 1, canvas - 1), radius=px(58), fill=255
    )
    image.alpha_composite(Image.composite(gradient, Image.new("RGBA", gradient.size), mask))
    draw = ImageDraw.Draw(image)

    # The open C-shaped ring is readable at every ICO size and represents a
    # quota window without looking like a calculator or a battery widget.
    arc_box = (px(45), px(45), px(211), px(211))
    draw.arc(arc_box, 42, 318, fill=WHITE, width=px(28))
    draw.arc(arc_box, 42, 128, fill=CYAN, width=px(28))
    draw.ellipse((px(190), px(112), px(220), px(142)), fill=LIME)

    return image.resize((size, size), Image.Resampling.LANCZOS)


def main() -> None:
    ICON_DIR.mkdir(parents=True, exist_ok=True)
    GITHUB_DIR.mkdir(parents=True, exist_ok=True)
    VERIFY_PATH.parent.mkdir(parents=True, exist_ok=True)

    rendered = {size: render_icon(size) for size in (16, 32, 48, 256)}
    for size, image in rendered.items():
        image.save(ICON_DIR / f"{size}x{size}.png", optimize=True)

    rendered[256].save(
        ICON_DIR / "icon.ico",
        format="ICO",
        sizes=[(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    rendered[256].save(GITHUB_DIR / "codex-usage-icon.png", optimize=True)

    sheet = Image.new("RGB", (760, 360), "#CBD5E1")
    draw = ImageDraw.Draw(sheet)
    for row, background in enumerate(("#F8FAFC", "#111827")):
        top = 20 + row * 170
        draw.rounded_rectangle((20, top, 740, top + 150), radius=18, fill=background)
        x = 55
        for size in (16, 32, 48, 128):
            icon = render_icon(size)
            sheet.paste(icon, (x, top + (150 - size) // 2), icon)
            x += size + 70
    sheet.save(VERIFY_PATH, optimize=True)


if __name__ == "__main__":
    main()
