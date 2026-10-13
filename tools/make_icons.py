#!/usr/bin/env python3
"""Draw the KeyRoam icon (a key on a teal rounded square) at any size.

    python3 tools/make_icons.py            # browser-extension icons
    python3 tools/make_icons.py --app      # also the desktop/mobile app icons

Needs Pillow. The artwork is drawn at 1024 px and downsampled, so every size
comes from the same geometry.
"""

import argparse
import os
import sys

from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TOP = (18, 163, 176)
BOTTOM = (11, 84, 102)
S = 1024


def master(rounded=True):
    grad = Image.new("RGBA", (S, S))
    px = grad.load()
    for y in range(S):
        t = y / (S - 1)
        c = tuple(round(TOP[i] + (BOTTOM[i] - TOP[i]) * t) for i in range(3))
        for x in range(S):
            px[x, y] = c + (255,)
    mask = Image.new("L", (S, S), 0)
    if rounded:
        ImageDraw.Draw(mask).rounded_rectangle((40, 40, S - 40, S - 40), radius=220, fill=255)
    else:
        mask.paste(255, (0, 0, S, S))
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    white = (255, 255, 255, 255)
    # Bow: a ring.
    cx, cy, r, w = 352, 512, 190, 92
    d.ellipse((cx - r, cy - r, cx + r, cy + r), fill=white)
    hole = r - w
    d.ellipse((cx - hole, cy - hole, cx + hole, cy + hole), fill=(0, 0, 0, 0))
    # Re-fill the hole with the background gradient.
    hole_mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(hole_mask).ellipse((cx - hole, cy - hole, cx + hole, cy + hole), fill=255)
    img.paste(grad, (0, 0), hole_mask)
    # Shaft and teeth.
    d.rounded_rectangle((cx + r - 30, cy - 46, 846, cy + 46), radius=30, fill=white)
    d.rounded_rectangle((700, cy + 20, 760, cy + 150), radius=18, fill=white)
    d.rounded_rectangle((786, cy + 20, 846, cy + 112), radius=18, fill=white)
    return img


def save(img, path, size):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    img.resize((size, size), Image.LANCZOS).save(path, optimize=True)


def extension_icons(img):
    for size in (16, 32, 48, 128):
        save(img, os.path.join(ROOT, "browser-extension/src/icons", f"icon-{size}.png"), size)


def app_icons(img, square):
    mac = os.path.join(ROOT, "app/macos/Runner/Assets.xcassets/AppIcon.appiconset")
    for size in (16, 32, 64, 128, 256, 512, 1024):
        save(img, os.path.join(mac, f"app_icon_{size}.png"), size)
    ios = os.path.join(ROOT, "app/ios/Runner/Assets.xcassets/AppIcon.appiconset")
    for name in os.listdir(ios):
        if name.endswith(".png"):
            with Image.open(os.path.join(ios, name)) as old:
                size = old.size[0]
            # iOS masks the corners itself and rejects transparency.
            save(square.convert("RGB"), os.path.join(ios, name), size)
    res = os.path.join(ROOT, "app/android/app/src/main/res")
    for density, size in (("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)):
        save(img, os.path.join(res, f"mipmap-{density}", "ic_launcher.png"), size)
    ico = os.path.join(ROOT, "app/windows/runner/resources/app_icon.ico")
    img.save(ico, sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--app", action="store_true", help="also write the app icons")
    args = p.parse_args()
    img = master()
    extension_icons(img)
    if args.app:
        app_icons(img, master(rounded=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
