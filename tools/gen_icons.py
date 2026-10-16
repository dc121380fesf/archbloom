# -*- coding: utf-8 -*-
"""
Archcore UI 图标套装生成器 —— Hana 家风
暖白纸面 #fffdfa / 黛青 #557477 / 浅底 tint #edf2ee
点缀：暖琥珀 #e2a45e / 淡粉 #f0c3bb；白色柔描边
形状语言：squircle 柔圆角、圆头线条、无尖锐工业感
产出：
  D:/Word/archcore/assets/icons/icon_{512,256,128,64,32}.png
  D:/Word/archcore/assets/icons/toolbar_*.png (64x64, 透明底)
"""
import math
import os
from PIL import Image, ImageDraw, ImageFilter

OUT = r"D:\Word\archcore\assets\icons"
PREVIEW_OUT = r"D:\Word\archcore\assets"
os.makedirs(OUT, exist_ok=True)

# ---------- Hana 色板 ----------
PAPER      = (255, 253, 250, 255)   # 暖白纸面
TINT       = (237, 242, 238, 255)   # 浅底
TEAL       = (85, 116, 119, 255)    # 黛青 主色
TEAL_DEEP  = (66, 96, 100, 255)     # 深黛
TEAL_SOFT  = (138, 168, 168, 255)   # 柔黛
TEAL_PALE  = (176, 199, 196, 255)   # 淡黛
AMBER      = (226, 164, 94, 255)    # 暖琥珀
AMBER_PALE = (240, 206, 166, 255)
PINK       = (240, 195, 187, 255)   # 淡粉
PINK_DEEP  = (228, 166, 158, 255)
WHITE_SOFT = (255, 255, 255, 235)


# ---------- 几何工具 ----------
def superellipse_points(cx, cy, ax, ay, n=4.2, steps=256):
    """squircle 采样点"""
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + ax * (1 if c >= 0 else -1) * abs(c) ** (2 / n)
        y = cy + ay * (1 if s >= 0 else -1) * abs(s) ** (2 / n)
        pts.append((x, y))
    return pts


def _bez(p0, p1, p2, t):
    u = 1 - t
    return (u * u * p0[0] + 2 * u * t * p1[0] + t * t * p2[0],
            u * u * p0[1] + 2 * u * t * p1[1] + t * t * p2[1])


def rounded_poly_points(pts, r, seg=8):
    """任意多边形的圆角化轮廓点"""
    out = []
    n = len(pts)
    for i in range(n):
        p0 = pts[(i - 1) % n]
        p1 = pts[i]
        p2 = pts[(i + 1) % n]
        v0 = (p0[0] - p1[0], p0[1] - p1[1])
        v1 = (p2[0] - p1[0], p2[1] - p1[1])
        l0 = math.hypot(*v0) or 1
        l1 = math.hypot(*v1) or 1
        rr = min(r, l0 / 2, l1 / 2)
        a = (p1[0] + v0[0] / l0 * rr, p1[1] + v0[1] / l0 * rr)
        b = (p1[0] + v1[0] / l1 * rr, p1[1] + v1[1] / l1 * rr)
        for s in range(seg + 1):
            out.append(_bez(a, p1, b, s / seg))
    return out


def rounded_ellipse_pts(cx, cy, rx, ry, rot=0.0, steps=64):
    pts = []
    cr, sr = math.cos(rot), math.sin(rot)
    for i in range(steps):
        t = 2 * math.pi * i / steps
        x, y = rx * math.cos(t), ry * math.sin(t)
        pts.append((cx + x * cr - y * sr, cy + x * sr + y * cr))
    return pts


def stroke_path(d, pts, width, color):
    """圆头折线"""
    d.line(pts, fill=color, width=width, joint="curve")
    r = width / 2
    for (x, y) in (pts[0], pts[-1]):
        d.ellipse([x - r, y - r, x + r, y + r], fill=color)


def rounded_rect(d, box, r, fill):
    d.rounded_rectangle(box, radius=r, fill=fill)


def rot_rect_pts(cx, cy, w, h, rot=0.0):
    cr, sr = math.cos(rot), math.sin(rot)
    pts = []
    for (x, y) in [(-w / 2, -h / 2), (w / 2, -h / 2), (w / 2, h / 2), (-w / 2, h / 2)]:
        pts.append((cx + x * cr - y * sr, cy + x * sr + y * cr))
    return pts


def star_pts(cx, cy, r_out, r_in, points=4, rot=-math.pi / 2):
    pts = []
    for i in range(points * 2):
        ang = rot + i * math.pi / points
        r = r_out if i % 2 == 0 else r_in
        pts.append((cx + math.cos(ang) * r, cy + math.sin(ang) * r))
    return pts


# ---------- 渐变 ----------
def v_gradient(size, top, bottom):
    w, h = size
    img = Image.new("RGBA", (w, h))
    dd = ImageDraw.Draw(img)
    for y in range(h):
        t = y / max(1, h - 1)
        c = tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(4))
        dd.line([(0, y), (w, y)], fill=c)
    return img


# =========================================================
# ① 应用主图标（品牌标）
# =========================================================
def build_app_icon(master=2048):
    SS = master / 1024.0            # 以 1024 设计稿为基准
    def u(v):                       # 设计单位 -> 像素
        return v * SS

    img = Image.new("RGBA", (master, master), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # --- squircle 底：暖白 -> 浅底轻渐变 ---
    grad = v_gradient((master, master), (255, 253, 250, 255), (233, 240, 235, 255))
    mask = Image.new("L", (master, master), 0)
    md = ImageDraw.Draw(mask)
    md.polygon(superellipse_points(master / 2, master / 2, master * 0.492, master * 0.492, n=4.3), fill=255)
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)

    # --- 取景框：四角柔括号（淡黛，含蓄）---
    inset, arm, w = u(120), u(112), u(15)
    lo, hi = inset, master - inset
    frame_col = (120, 152, 152, 150)
    for (x, y, dx, dy) in [(lo, lo, 1, 1), (hi, lo, -1, 1), (lo, hi, 1, -1), (hi, hi, -1, -1)]:
        pts = [(x + dx * arm, y), (x, y), (x, y + dy * arm)]
        stroke_path(d, [(p[0], p[1]) for p in pts], int(w), frame_col)

    # --- 柔化建筑体块（底部一条柔地平）---
    base_y = u(872)
    d.rounded_rectangle([u(200), base_y - u(6), u(824), base_y + u(10)], radius=u(8),
                        fill=(176, 199, 196, 120))

    # 低体块（左，深黛）
    d.rounded_rectangle([u(232), u(588), u(392), base_y], radius=u(38), fill=TEAL_DEEP)
    # 高体块（中，主色黛青）
    d.rounded_rectangle([u(352), u(330), u(608), base_y], radius=u(44), fill=TEAL)
    # 中体块（右，柔黛）
    d.rounded_rectangle([u(632), u(508), u(800), base_y], radius=u(40), fill=TEAL_SOFT)

    # 窗（暖白柔方，体块呼吸感；小尺寸糊成亮斑仍悦目）
    for (wx, wy) in [(u(398), u(398)), (u(500), u(398)), (u(398), u(500)), (u(500), u(500)),
                     (u(398), u(602)), (u(500), u(602))]:
        d.rounded_rectangle([wx, wy, wx + u(58), wy + u(58)], radius=u(16), fill=(255, 253, 250, 225))
    # 右体块的拱形取景门（建筑的"取景"之意，落地拱门）
    d.rounded_rectangle([u(676), u(596), u(760), base_y], radius=u(42),
                        fill=(255, 253, 250, 225), corners=(True, True, False, False))

    # --- 花朵点缀：五瓣柔花 + 一叶，栖在高体块肩头 ---
    fx, fy = u(648), u(286)
    petal_col = PINK
    for k in range(5):
        ang = -math.pi / 2 + k * 2 * math.pi / 5
        px = fx + math.cos(ang) * u(72)
        py = fy + math.sin(ang) * u(72)
        pts = rounded_ellipse_pts(px, py, u(52), u(78), rot=ang + math.pi / 2)
        d.polygon(pts, fill=petal_col)
    d.ellipse([fx - u(36), fy - u(36), fx + u(36), fy + u(36)], fill=AMBER)
    d.ellipse([fx - u(15), fy - u(15), fx + u(15), fy + u(15)], fill=AMBER_PALE)

    # 枝与叶（从高体块顶生出，柔线）
    stem = [(u(560), u(350)), (u(578), u(312)), (u(612), u(292))]
    stroke_path(d, stem, int(u(13)), (110, 146, 132, 255))
    leaf = rounded_ellipse_pts(u(548), u(300), u(52), u(24), rot=math.radians(-38))
    d.polygon(leaf, fill=(126, 164, 138, 255))

    # --- 白色柔描边（图片衬垫气质）---
    outline = superellipse_points(master / 2, master / 2, master * 0.487, master * 0.487, n=4.3)
    d.line(outline + outline[:1], fill=WHITE_SOFT, width=int(u(11)), joint="curve")

    # 轻微柔焦，气质更雅
    img = img.filter(ImageFilter.GaussianBlur(u(1.2)))

    sizes = [512, 256, 128, 64, 32]
    for s in sizes:
        out = img.resize((s, s), Image.LANCZOS)
        out.save(os.path.join(OUT, f"icon_{s}.png"))
    print("app icon done:", sizes)


# =========================================================
# ② 工具栏图标组（64x64 透明底，柔面性：柔面 + 圆头细线，暖白/琥珀点缀）
# =========================================================
SS = 8  # 512 画布 -> 64
BG_DARK = (36, 52, 56, 255)


def toolbar_canvas():
    return Image.new("RGBA", (64 * SS, 64 * SS), (0, 0, 0, 0))


def draw_new(d):
    u = SS
    d.rounded_rectangle([17 * u, 11 * u, 47 * u, 53 * u], radius=8 * u, fill=TEAL)
    d.rounded_rectangle([21 * u, 15 * u, 43 * u, 49 * u], radius=5 * u,
                        fill=(255, 253, 250, 40))
    stroke_path(d, [(32 * u, 22 * u), (32 * u, 42 * u)], int(4.6 * u), PAPER)
    stroke_path(d, [(22 * u, 32 * u), (42 * u, 32 * u)], int(4.6 * u), PAPER)
    d.ellipse([41 * u, 10 * u, 47 * u, 16 * u], fill=AMBER)  # 琥珀小点


def draw_open(d):
    u = SS
    # 后片 + 页签（文件夹背）
    d.rounded_rectangle([12 * u, 15 * u, 30 * u, 27 * u], radius=5 * u, fill=TEAL_SOFT)
    d.rounded_rectangle([12 * u, 21 * u, 52 * u, 52 * u], radius=8 * u, fill=TEAL_SOFT)
    # 探出的柔白图纸（微微倾斜）
    d.polygon(rounded_poly_points(rot_rect_pts(33 * u, 25 * u, 24 * u, 22 * u, math.radians(-9)), 3 * u),
              fill=PAPER)
    stroke_path(d, [(27 * u, 22 * u), (38 * u, 21 * u)], int(1.8 * u), TEAL_PALE)
    stroke_path(d, [(27 * u, 26 * u), (36 * u, 25 * u)], int(1.8 * u), TEAL_PALE)
    # 前片（打开的盖）
    d.polygon(rounded_poly_points([(10 * u, 31 * u), (54 * u, 31 * u), (48 * u, 52 * u),
                                   (16 * u, 52 * u)], 6 * u),
              fill=TEAL)
    d.ellipse([45 * u, 43 * u, 50 * u, 48 * u], fill=AMBER_PALE)


def draw_save(d):
    u = SS
    d.rounded_rectangle([13 * u, 12 * u, 51 * u, 52 * u], radius=9 * u, fill=TEAL)
    # 标签槽
    d.rounded_rectangle([23 * u, 12 * u, 41 * u, 27 * u], radius=4 * u, fill=PAPER)
    d.rounded_rectangle([33 * u, 15 * u, 39 * u, 24 * u], radius=2 * u, fill=TEAL_SOFT)
    # 盘面
    d.rounded_rectangle([21 * u, 32 * u, 43 * u, 52 * u], radius=5 * u, fill=PAPER)
    d.rounded_rectangle([25 * u, 36 * u, 39 * u, 44 * u], radius=3 * u, fill=TEAL_SOFT)
    d.ellipse([43 * u, 44 * u, 49 * u, 50 * u], fill=AMBER)


def draw_render(d):
    u = SS
    cx, cy, r = 30 * u, 35 * u, 19 * u
    # 等轴柔立方：三面（明度递进，深浅底皆立得住）
    top = [(cx, cy - r), (cx + r * 0.92, cy - r * 0.48), (cx, cy), (cx - r * 0.92, cy - r * 0.48)]
    left = [(cx - r * 0.92, cy - r * 0.40), (cx, cy + r * 0.12), (cx, cy + r), (cx - r * 0.92, cy + r * 0.48)]
    right = [(cx + r * 0.92, cy - r * 0.40), (cx, cy + r * 0.12), (cx, cy + r), (cx + r * 0.92, cy + r * 0.48)]
    d.polygon(rounded_poly_points(top, 5 * u), fill=TEAL_PALE)
    d.polygon(rounded_poly_points(left, 5 * u), fill=TEAL_DEEP)
    d.polygon(rounded_poly_points(right, 5 * u), fill=TEAL)
    # 琥珀四角星芒 ×2（渲染之光，圆角星，绝不像加号）
    d.polygon(rounded_poly_points(star_pts(48 * u, 16 * u, 10 * u, 3.4 * u), 1.6 * u), fill=AMBER)
    d.polygon(rounded_poly_points(star_pts(40 * u, 11 * u, 5.2 * u, 1.8 * u), 1.0 * u), fill=AMBER_PALE)


def draw_pano(d):
    u = SS
    d.rounded_rectangle([8 * u, 17 * u, 56 * u, 47 * u], radius=9 * u, fill=TEAL)
    d.rounded_rectangle([12 * u, 21 * u, 52 * u, 43 * u], radius=6 * u, fill=PAPER)
    # 远山（柔丘）
    d.polygon(rounded_poly_points([(12 * u, 43 * u), (22 * u, 30 * u), (32 * u, 43 * u)], 5 * u),
              fill=TEAL_SOFT)
    d.polygon(rounded_poly_points([(28 * u, 43 * u), (40 * u, 28 * u), (52 * u, 43 * u)], 5 * u),
              fill=TEAL)
    # 落日
    d.ellipse([40 * u, 23 * u, 47 * u, 30 * u], fill=AMBER)
    # 地平柔线
    stroke_path(d, [(13 * u, 42.4 * u), (51 * u, 42.4 * u)], int(2.2 * u), PAPER)


def draw_walk(d):
    u = SS
    col = TEAL
    # 头（柔面圆）
    d.ellipse([27 * u, 9 * u, 39 * u, 21 * u], fill=col)
    # 身与四肢（圆头柔线，与柔面图标同重量）
    stroke_path(d, [(33 * u, 23 * u), (31 * u, 37 * u)], int(6.6 * u), col)      # 躯干
    stroke_path(d, [(31 * u, 28 * u), (22 * u, 34 * u)], int(5.8 * u), col)      # 左臂
    stroke_path(d, [(31 * u, 28 * u), (42 * u, 25 * u)], int(5.8 * u), col)      # 右臂
    stroke_path(d, [(31 * u, 37 * u), (24 * u, 51 * u)], int(6.2 * u), col)      # 左腿
    stroke_path(d, [(31 * u, 37 * u), (40 * u, 49 * u)], int(6.2 * u), col)      # 右腿
    # 脚下柔地线（漫游之径）
    stroke_path(d, [(12 * u, 56 * u), (24 * u, 56 * u)], int(3.4 * u), TEAL_PALE)
    stroke_path(d, [(33 * u, 56 * u), (52 * u, 56 * u)], int(3.4 * u), TEAL_PALE)
    d.ellipse([45 * u, 8 * u, 51 * u, 14 * u], fill=AMBER)


def draw_review(d):
    u = SS
    # 柔对话框
    d.rounded_rectangle([10 * u, 13 * u, 54 * u, 44 * u], radius=11 * u, fill=TEAL)
    d.polygon(rounded_poly_points([(22 * u, 42 * u), (18 * u, 53 * u), (33 * u, 42 * u)], 5 * u),
              fill=TEAL)
    # 勾选（暖白）
    stroke_path(d, [(22 * u, 28 * u), (29 * u, 35 * u), (42 * u, 21 * u)], int(4.8 * u), PAPER)
    d.ellipse([45 * u, 12 * u, 51 * u, 18 * u], fill=AMBER)


def draw_settings(d):
    u = SS
    cx, cy = 32 * u, 32 * u
    # 柔齿轮：8 枚圆角齿（无尖角，花与齿轮之间）
    for k in range(8):
        ang = k * math.pi / 4
        tx = cx + math.cos(ang) * 17.5 * u
        ty = cy + math.sin(ang) * 17.5 * u
        d.polygon(rounded_poly_points(rot_rect_pts(tx, ty, 9 * u, 9 * u, ang), 3.2 * u), fill=TEAL)
    d.ellipse([cx - 17 * u, cy - 17 * u, cx + 17 * u, cy + 17 * u], fill=TEAL)
    # 柔凹槽 + 琥珀芯
    d.ellipse([cx - 8.5 * u, cy - 8.5 * u, cx + 8.5 * u, cy + 8.5 * u], fill=PAPER)
    d.ellipse([cx - 3.2 * u, cy - 3.2 * u, cx + 3.2 * u, cy + 3.2 * u], fill=AMBER)


TOOLBAR = {
    "toolbar_new": draw_new,
    "toolbar_open": draw_open,
    "toolbar_save": draw_save,
    "toolbar_render": draw_render,
    "toolbar_pano": draw_pano,
    "toolbar_walk": draw_walk,
    "toolbar_review": draw_review,
    "toolbar_settings": draw_settings,
}


def build_toolbar():
    previews = []
    for name, fn in TOOLBAR.items():
        canvas = toolbar_canvas()
        d = ImageDraw.Draw(canvas)
        fn(d)
        canvas = canvas.filter(ImageFilter.GaussianBlur(0.6 * SS / 4))
        icon = canvas.resize((64, 64), Image.LANCZOS)
        icon.save(os.path.join(OUT, f"{name}.png"))
        previews.append((name, icon))
    print("toolbar done:", len(previews))
    return previews


# =========================================================
# 预览图（浅底 / 深底对照，交付展示用）
# =========================================================
def build_preview(previews):
    W, H = 1240, 760
    sheet = Image.new("RGBA", (W, H), PAPER)
    sd = ImageDraw.Draw(sheet)

    # 浅底区 / 深底区
    sd.rounded_rectangle([40, 250, 1200, 720], radius=28, fill=TINT)
    sd.rounded_rectangle([40, 490, 1200, 720], radius=28, fill=(40, 58, 62, 255))

    # 主图标一排
    x = 70
    for s in [256, 128, 64, 32]:
        p = os.path.join(OUT, f"icon_{s}.png")
        im = Image.open(p).convert("RGBA")
        sheet.paste(im, (x, 60 + (256 - s) // 2), im)
        x += s + 46

    # 工具栏：浅底行
    x = 70
    for name, im in previews:
        big = im.resize((64, 64), Image.LANCZOS)
        sheet.paste(big, (x, 330), big)
        sd.text((x + 6, 410), name.replace("toolbar_", ""), fill=(90, 116, 118, 255))
        x += 142

    # 工具栏：深底行
    x = 70
    for name, im in previews:
        big = im.resize((64, 64), Image.LANCZOS)
        sheet.paste(big, (x, 560), big)
        x += 142

    sheet = sheet.convert("RGB")
    sheet.save(os.path.join(PREVIEW_OUT, "archcore_icons_preview.png"))
    print("preview saved")


if __name__ == "__main__":
    build_app_icon()
    prev = build_toolbar()
    build_preview(prev)
    print("ALL DONE ->", OUT)
