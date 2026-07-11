"""Generate deterministic PNG and ICO assets from the Codex Usage icon geometry."""

from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
ICON_DIR = ROOT / "src" / "icons"
GITHUB_DIR = ROOT / ".github"
VERIFY_PATH = ROOT / "target" / "codex-usage-icon-verification.png"

NAVY = "#0B1220"
CYAN = "#38BDF8"
LIME = "#A3E635"
PALE = "#E2E8F0"


def render_icon(size: int) -> Image.Image:
    scale = max(4, 1024 // size)
    canvas = size * scale
    image = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    def px(value: float) -> int:
        return round(value * canvas / 256)

    draw.rounded_rectangle((0, 0, canvas - 1, canvas - 1), radius=px(56), fill=NAVY)
    arc_box = (px(42), px(42), px(214), px(214))
    draw.arc(arc_box, 45, 315, fill=CYAN, width=px(26))
    draw.arc(arc_box, 318, 350, fill=LIME, width=px(26))

    bar_width = px(18)
    for y, end, color in ((100, 162, PALE), (128, 144, CYAN), (156, 126, LIME)):
        draw.line((px(100), px(y), px(end), px(y)), fill=color, width=bar_width)
        radius = bar_width // 2
        for x in (px(100), px(end)):
            draw.ellipse((x - radius, px(y) - radius, x + radius, px(y) + radius), fill=color)

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
