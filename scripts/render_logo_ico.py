"""Rasterize the Zicade logo (crates/zicade-web/assets/logo.svg) to a multi-size
.ico. The SVG is simple geometry, so we redraw its exact primitives in Pillow at
high resolution and downsample for anti-aliasing."""

import sys
from PIL import Image, ImageDraw

VIEW = 64                    # SVG viewBox is 0..64
M = 1024                     # master render size (px)
S = M / VIEW                 # units -> px

TOP = (0x4f, 0x46, 0xe5)     # gradient top   #4f46e5
BOT = (0x25, 0x63, 0xeb)     # gradient bottom #2563eb
WHITE = (0xff, 0xff, 0xff)
NODE_OUTER = (0x25, 0x63, 0xeb)   # #2563eb
NODE_INNER = (0x22, 0xd3, 0xee)   # #22d3ee cyan


def u(v):  # unit -> master px
    return v * S


def circle(draw, cx, cy, r, fill):
    draw.ellipse([u(cx) - u(r), u(cy) - u(r), u(cx) + u(r), u(cy) + u(r)], fill=fill)


def render_master():
    # 1) Vertical gradient panel.
    grad = Image.new("RGB", (M, M), TOP)
    px = grad.load()
    for y in range(M):
        t = y / (M - 1)
        col = tuple(round(TOP[i] + (BOT[i] - TOP[i]) * t) for i in range(3))
        for x in range(M):
            px[x, y] = col

    # 2) Rounded-rect mask: badge rect x=3 y=3 w=58 h=58 rx=15.
    mask = Image.new("L", (M, M), 0)
    md = ImageDraw.Draw(mask)
    md.rounded_rectangle([u(3), u(3), u(3 + 58), u(3 + 58)], radius=u(15), fill=255)

    img = Image.new("RGBA", (M, M), (0, 0, 0, 0))
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)

    # 3) The "Z" forwarding path: M19 20 L45 20 L19 44 L45 44, width 6, round joins.
    pts = [(u(19), u(20)), (u(45), u(20)), (u(19), u(44)), (u(45), u(44))]
    d.line(pts, fill=WHITE, width=round(u(6)), joint="curve")
    # Round the two open ends (Pillow line caps are flat).
    for cx, cy in [(19, 20), (45, 44)]:
        circle(d, cx, cy, 3, WHITE)

    # 4) Endpoint nodes at the two loose ends.
    for cx, cy in [(19, 20), (45, 44)]:
        circle(d, cx, cy, 5.2, NODE_OUTER)
        circle(d, cx, cy, 3.3, NODE_INNER)

    return img


def main(out):
    master = render_master()
    sizes = [16, 24, 32, 48, 64, 128, 256]
    base = master.resize((256, 256), Image.LANCZOS)  # ICO max is 256; source must be largest
    base.save(out, format="ICO", sizes=[(s, s) for s in sizes])
    base.save(out.replace(".ico", "_preview.png"))
    print("wrote", out)


if __name__ == "__main__":
    main(sys.argv[1])
