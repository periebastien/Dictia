"""Génère assets/dictia.ico (logo : carré ambre, micro sombre).

Usage : python3 assets/generate_icon.py   (nécessite Pillow)
"""
from pathlib import Path

from PIL import Image, ImageDraw

ACCENT = (0xF5, 0x9E, 0x0B, 255)  # ambre, bien visible dans la barre des tâches
INK = (0x0E, 0x0E, 0x12, 255)
BIG = 1024
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]


def draw_logo(size: int = BIG) -> Image.Image:
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    margin = size * 0.04
    d.rounded_rectangle(
        (margin, margin, size - margin, size - margin), radius=size * 0.22, fill=ACCENT
    )

    # Micro sur une grille de 24 unités (comme l'icône des maquettes).
    unit = size * 0.62 / 24
    off = (size - 24 * unit) / 2

    def p(x, y):
        return off + x * unit, off + y * unit

    stroke = 2.6 * unit
    # Capsule pleine.
    d.rounded_rectangle((*p(8, 1.2), *p(16, 15.4)), radius=4 * unit, fill=INK)
    # Arceau : demi-cercle bas + deux montants.
    d.arc((*p(5, 5), *p(19, 19)), start=0, end=180, fill=INK, width=round(stroke))
    for x in (5, 19):
        x0 = p(x, 0)[0]
        d.rectangle((x0 - stroke / 2, p(0, 9.5)[1], x0 + stroke / 2, p(0, 12.2)[1]), fill=INK)
        cx, cy = p(x, 9.5)
        d.ellipse((cx - stroke / 2, cy - stroke / 2, cx + stroke / 2, cy + stroke / 2), fill=INK)
    # Pied.
    cx = p(12, 0)[0]
    d.rectangle((cx - stroke / 2, p(0, 19)[1], cx + stroke / 2, p(0, 22.4)[1]), fill=INK)
    cy = p(0, 22.4)[1]
    d.ellipse((cx - stroke / 2, cy - stroke / 2, cx + stroke / 2, cy + stroke / 2), fill=INK)
    return img


def main() -> None:
    out = Path(__file__).with_name("dictia.ico")
    logo = draw_logo()
    frames = [logo.resize((s, s), Image.LANCZOS) for s in SIZES]
    frames[-1].save(out, format="ICO", sizes=[(s, s) for s in SIZES], append_images=frames[:-1])
    logo.resize((256, 256), Image.LANCZOS).save(out.with_name("dictia.png"))
    print(f"{out} ({out.stat().st_size} octets)")


if __name__ == "__main__":
    main()
