#!/usr/bin/env python3
"""精致二层小洋楼生成器：手写几何装配（坡屋顶/老虎窗/烟囱/柱廊/阳台/线脚/玻璃），
输出 demo/villa.json 由 archcore-run 执行。id 经计数器精确追踪（opening.cut 按 id 引墙）。"""
import json

ops = []
counter = [0]
IDS = {}


def alloc(n=1):
    out = []
    for _ in range(n):
        counter[0] += 1
        out.append(f"obj-{counter[0]}")
    return out


def op(name, args):
    ops.append({"op": name, "args": args})


def mat(mid, name, color, metallic=0.0, rough=0.8):
    op("material.create", {"name": name, "id": mid, "base_color": color + [1.0],
                           "metallic": metallic, "roughness": rough})


BOX_IDX = [0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4,
           2, 3, 7, 2, 7, 6, 1, 2, 6, 1, 6, 5, 0, 4, 7, 0, 7, 3]


def box_verts(x0, y0, z0, x1, y1, z1):
    return [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0],
            [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]]


def merge_boxes(boxes):
    pos, idx = [], []
    for i, (a, b) in enumerate(boxes):
        base = len(pos)
        pos.extend(box_verts(a[0], a[1], a[2], b[0], b[1], b[2]))
        idx.extend([j + base for j in BOX_IDX])
    return pos, idx


def merge_boxes_uv(boxes, mode, scale):
    pos, idx, uvs = [], [], []
    for (a, b) in boxes:
        base = len(pos)
        vs = box_verts(a[0], a[1], a[2], b[0], b[1], b[2])
        pos.extend(vs)
        idx.extend([j + base for j in BOX_IDX])
        for p in vs:
            if mode == "xy":
                uvs.append([p[0] / scale, p[1] / scale])
            elif mode == "xz":
                uvs.append([p[0] / scale, p[2] / scale])
            else:
                uvs.append([p[1] / scale, p[2] / scale])
    return pos, idx, uvs


def mesh(name, pos, idx, material, uv=None):
    [oid] = alloc(1)
    a = {"name": name, "positions": pos, "indices": idx, "material": material}
    if uv:
        a["uv"] = uv
    op("mesh.create", a)
    IDS[name] = oid
    return oid


def mesh_box(name, a, b, material, uv_mode=None, uv_scale=2.5):
    if uv_mode:
        p, i, u = merge_boxes_uv([(a, b)], uv_mode, uv_scale)
        return mesh(name, p, i, material, u)
    p, i = merge_boxes([(a, b)])
    return mesh(name, p, i, material)


def furniture(name, kind, corner, size=None, rot=0.0, material=None):
    [oid] = alloc(1)
    a = {"kind": kind, "name": name, "position": corner, "material": material}
    if size:
        a["size"] = size
    if rot:
        a["rotate_z"] = rot
    op("furniture.create", a)
    return oid


def camera(name, pos, target, lens=35):
    [oid] = alloc(1)
    op("camera.create", {"name": name, "translation": pos, "target": target, "lens_mm": lens})
    return oid


def cut(wall, kind, center_x_or_y, wall_axis_val, z0, w, h):
    """opening.cut：pos = 洞口底边中点（横向中心在 u 轴）"""
    op("opening.cut", {"wall": wall, "kind": kind,
                       "position": [center_x_or_y, wall_axis_val, z0],
                       "width": w, "height": h})


# ============================== 材质 ==============================
mat("mat-0", "MAT_Stucco", [0.93, 0.885, 0.80])          # 暖白灰泥
mat("mat-1", "MAT_Terracotta", [0.60, 0.215, 0.115], 0.0, 0.85)  # 陶土瓦
mat("mat-2", "MAT_TrimWhite", [0.965, 0.955, 0.925], 0.0, 0.6)   # 白色线脚
mat("mat-3", "MAT_Wood", [0.42, 0.26, 0.15], 0.0, 0.65)          # 木作
mat("mat-4", "MAT_Glass", [0.42, 0.50, 0.54], 1.0, 0.15)         # 镜面假玻璃（金属度 1 → mirror 路）
mat("mat-5", "MAT_Stone", [0.56, 0.53, 0.485], 0.0, 0.9)         # 勒脚/台阶石
mat("mat-6", "MAT_Ground", [0.52, 0.585, 0.475], 0.0, 0.95)      # 草地

# 质感层：程序化贴图上身（贴图 × base_color 色调）
for mid, fname in [("mat-0", "stucco_wall"), ("mat-1", "roof_tiles"),
                   ("mat-3", "wood_floor"), ("mat-5", "stone_block"), ("mat-6", "grass")]:
    op("material.set_texture", {"material": mid, "slot": "base_color",
                                "uri": f"D:/Word/archcore/demo/tex/{fname}.jpg"})

# ============================== 场地 ==============================
mesh_box("Ground", (-10, -9, -0.28), (24, 18, 0.0), "mat-6", "xy", 6.0)
mesh_box("Walkway", (4.05, -5.2, 0.0), (5.95, 0.0, 0.1), "mat-5", "xy", 2.0)
mesh_box("Plinth", (-0.16, -0.16, 0.0), (10.16, 8.16, 0.45), "mat-5", "xy", 3.0)  # 勒脚

# ============================== 墙体（0.24 薄墙，两层分段） ==============================
WT = 0.24
F1Z0, F1Z1 = 0.45, 3.35
BAND = (3.35, 3.60)
F2Z0, F2Z1 = 3.60, 6.40

def wall_panels(name, a0, a1, b_lo, b_hi, axis):
    """axis='y'：墙沿 x 展开（前/后）；axis='x'：沿 y 展开（左/右）"""
    out = []
    for tag, z0, z1 in [("F1", F1Z0, F1Z1), ("F2", F2Z0, F2Z1)]:
        if axis == "y":
            out.append(mesh_box(f"{name}_{tag}", (a0, b_lo, z0), (a1, b_hi, z1), "mat-0", "xz", 2.5))
        else:
            out.append(mesh_box(f"{name}_{tag}", (b_lo, a0, z0), (b_hi, a1, z1), "mat-0", "yz", 2.5))
    return out

front = wall_panels("WallFront", 0.0, 10.0, 0.0, WT, "y")
back = wall_panels("WallBack", 0.0, 10.0, 8.0 - WT, 8.0, "y")
left = wall_panels("WallLeft", 0.0, 8.0, 0.0, WT, "x")
right = wall_panels("WallRight", 0.0, 8.0, 10.0 - WT, 10.0, "x")

# 楼板（二层结构板，窗内可见进深）
mesh_box("Floor2Slab", (WT, WT, 3.22), (10.0 - WT, 8.0 - WT, 3.36), "mat-2")
# 内隔断（透过门窗显层次）
mesh_box("Partition", (3.2, WT, F1Z0), (3.44, 4.2, F1Z1), "mat-0")

# ============================== 门窗开洞 ==============================
DOOR_W, DOOR_H = 1.15, 2.30
WIN_W, WIN_H = 1.30, 1.70
WIN2_W, WIN2_H = 1.10, 1.55
openings_front = []  # (cx, z0, w, h, kind)

# 前墙一层：大门 + 双窗
cut(front[0], "door", 5.0, 0.12, F1Z0, DOOR_W, DOOR_H)
openings_front.append((5.0, F1Z0, DOOR_W, DOOR_H, "door"))
for cx in (2.2, 7.8):
    cut(front[0], "window", cx, 0.12, 1.05, WIN_W, WIN_H)
    openings_front.append((cx, 1.05, WIN_W, WIN_H, "window"))
# 前墙二层：阳台门 + 双窗
cut(front[1], "door", 5.0, 0.12, F2Z0, 1.0, 2.15)
openings_front.append((5.0, F2Z0, 1.0, 2.15, "door"))
for cx in (2.2, 7.8):
    cut(front[1], "window", cx, 0.12, 4.35, WIN_W, WIN_H)
    openings_front.append((cx, 4.35, WIN_W, WIN_H, "window"))

# 侧墙/后墙：每层两窗
side_windows = []  # (axis, coord, z0, w, h)
for wall_pair, axis in ((left, "x"), (right, "x")):
    for panel in wall_pair:
        z0 = 1.05 if panel == wall_pair[0] else 4.35
        for cy in (2.7, 5.3):
            cut(panel, "window", 0.12, cy, z0, WIN2_W, WIN2_H)
            side_windows.append((axis, cy, z0, WIN2_W, WIN2_H, wall_pair is left))
for panel in back:
    z0 = 1.05 if panel == back[0] else 4.35
    for cx in (3.0, 7.0):
        cut(panel, "window", cx, 7.88, z0, WIN2_W, WIN2_H)

# ============================== 门窗框 + 玻璃 ==============================
for cx, z0, w, h, kind in openings_front:
    if kind == "door":
        furniture("门框", "door", (cx - (w + 0.18) / 2, -0.07, z0), [w + 0.18, 0.38, h + 0.14], 0.0, "mat-3")
    else:
        furniture("前窗框", "window", (cx - (w + 0.16) / 2, -0.07, z0 - 0.06), [w + 0.16, 0.38, h + 0.12], 0.0, "mat-2")

# 玻璃（镜面）：所有窗洞内嵌薄片（合并为单 mesh）
glass = []
for cx, z0, w, h, kind in openings_front:
    if kind == "window":
        glass.append(((cx - w / 2 + 0.05, 0.055, z0 + 0.05), (cx + w / 2 - 0.05, 0.185, z0 + h - 0.05)))
for axis, cy, z0, w, h, is_left in side_windows:
    x0, x1 = (0.055, 0.185) if is_left else (10 - 0.185, 10 - 0.055)
    furniture("侧窗框", "window", (x0 - 0.06, cy - (w + 0.16) / 2, z0 - 0.06), [0.38, w + 0.16, h + 0.12], 0.0, "mat-2")
    glass.append(((x0, cy - w / 2 + 0.05, z0 + 0.05), (x1, cy + w / 2 - 0.05, z0 + h - 0.05)))
for panel_i, panel in enumerate(back):
    z0 = 1.05 if panel_i == 0 else 4.35
    for cx in (3.0, 7.0):
        furniture("后窗框", "window", (cx - (WIN2_W + 0.16) / 2, 8 - 0.31, z0 - 0.06), [WIN2_W + 0.16, 0.38, WIN2_H + 0.12], 0.0, "mat-2")
        glass.append(((cx - WIN2_W / 2 + 0.05, 7.815, z0 + 0.05), (cx + WIN2_W / 2 - 0.05, 7.945, z0 + WIN2_H - 0.05)))
gp, gi = merge_boxes(glass)
mesh("Glass", gp, gi, "mat-4")

# ============================== 线脚：腰线 / 檐口 / 角石 / 窗台 ==============================
band_boxes = [((-0.07, -0.07, BAND[0]), (10.07, 0.31, BAND[1])),
              ((-0.07, 8 - 0.31, BAND[0]), (10.07, 8.07, BAND[1])),
              ((-0.07, 0.31, BAND[0]), (0.31, 8 - 0.31, BAND[1])),
              ((10 - 0.31, 0.31, BAND[0]), (10.07, 8 - 0.31, BAND[1]))]
bp, bi = merge_boxes(band_boxes)
mesh("StringCourse", bp, bi, "mat-2")

cornice_boxes = [((-0.14, -0.14, 6.40), (10.14, 0.38, 6.72)),
                 ((-0.14, 8 - 0.38, 6.40), (10.14, 8.14, 6.72)),
                 ((-0.14, 0.38, 6.40), (0.38, 8 - 0.38, 6.72)),
                 ((10 - 0.38, 0.38, 6.40), (10.14, 8 - 0.38, 6.72))]
cp, ci = merge_boxes(cornice_boxes)
mesh("Cornice", cp, ci, "mat-2")

quoin_boxes = []
for cx, cy in ((0, 0), (10, 0), (0, 8), (10, 8)):
    sx = -1 if cx == 0 else 1
    sy = -1 if cy == 0 else 1
    quoin_boxes.append(((min(cx, cx + sx * 0.5), min(cy, cy + sy * 0.5), 0.45),
                        (max(cx, cx + sx * 0.5), max(cy, cy + sy * 0.5), 6.40)))
qp, qi = merge_boxes(quoin_boxes)
mesh("Quoins", qp, qi, "mat-2")

sills = []
for cx, z0, w, h, kind in openings_front:
    if kind == "window":
        sills.append(((cx - w / 2 - 0.14, -0.10, z0 - 0.10), (cx + w / 2 + 0.14, 0.34, z0)))
for axis, cy, z0, w, h, is_left in side_windows:
    x0, x1 = (-0.10, 0.34) if is_left else (10 - 0.34, 10 + 0.10)
    sills.append(((x0, cy - w / 2 - 0.14, z0 - 0.10), (x1, cy + w / 2 + 0.14, z0)))
sp, si = merge_boxes(sills)
mesh("Sills", sp, si, "mat-2")

# ============================== 门廊：台阶 / 柱廊 / 檐篷 ==============================
mesh_box("Step1", (2.8, -2.75, 0.0), (7.2, -1.75, 0.18), "mat-5")
mesh_box("Step2", (3.0, -2.25, 0.18), (7.0, -1.75, 0.36), "mat-5")
mesh_box("PorchSlab", (3.15, -1.75, 0.36), (6.85, 0.0, 0.52), "mat-2")

col_boxes = []
for cx in (3.62, 6.38):
    col_boxes.append(((cx - 0.23, -1.42, 0.52), (cx + 0.23, -0.96, 0.70)))   # 柱础
    col_boxes.append(((cx - 0.16, -1.35, 0.70), (cx + 0.16, -1.03, 3.16)))   # 柱身
    col_boxes.append(((cx - 0.23, -1.42, 3.16), (cx + 0.23, -0.96, 3.35)))   # 柱头
cpo, cpi = merge_boxes(col_boxes)
mesh("PorchColumns", cpo, cpi, "mat-2")

mesh_box("Canopy", (3.0, -1.85, 3.35), (7.0, 0.24, 3.58), "mat-2")
mesh_box("Frieze", (2.9, -1.95, 3.58), (7.1, 0.30, 3.82), "mat-2")

# ============================== 阳台（檐篷之上） ==============================
mesh_box("BalconySlab", (3.15, -1.80, 3.82), (6.85, 0.10, 4.02), "mat-2")
rail_boxes = []
def railing_run(x0, y0, x1, y1):
    """沿段布栏杆柱 + 顶扶手"""
    dx, dy = x1 - x0, y1 - y0
    ln = (dx * dx + dy * dy) ** 0.5
    n = max(2, int(ln / 0.42) + 1)
    for i in range(n):
        t = i / (n - 1)
        cx, cy = x0 + dx * t, y0 + dy * t
        rail_boxes.append(((cx - 0.035, cy - 0.035, 4.02), (cx + 0.035, cy + 0.035, 4.97)))
    rail_boxes.append(((min(x0, x1) - 0.055, min(y0, y1) - 0.055, 4.97),
                       (max(x0, x1) + 0.055, max(y0, y1) + 0.055, 5.12)))
railing_run(3.15, -1.80, 6.85, -1.80)
railing_run(3.15, -1.80, 3.15, 0.10)
railing_run(6.85, -1.80, 6.85, 0.10)
rp, ri = merge_boxes(rail_boxes)
mesh("Balustrade", rp, ri, "mat-2")

# ============================== 屋顶（手写双坡棱柱 + 悬挑） ==============================
OV, RB, RH = 0.55, 6.72, 2.50
ax0, ax1 = -OV, 10.0 + OV
ay0, ay1 = -OV, 8.0 + OV
ridge_y = 4.0
roof_pos = [[ax0, ay0, RB], [ax1, ay0, RB], [5.0, ay0, RB + RH],
            [ax0, ay1, RB], [ax1, ay1, RB], [5.0, ay1, RB + RH]]
roof_idx = [0, 1, 2,            # 前山墙
            5, 4, 3,            # 后山墙
            0, 2, 5, 0, 5, 3,   # 前坡
            2, 1, 4, 2, 4, 5,   # 后坡
            0, 3, 4, 0, 4, 1]   # 底面
roof_uv = [[p[0] / 3.5, p[1] / 3.5] for p in roof_pos]
mesh("Roof", roof_pos, roof_idx, "mat-1", roof_uv)
mesh_box("RidgeCap", (4.9, -0.50, RB + RH), (5.1, 8.50, RB + RH + 0.16), "mat-1")

# 山墙尖（gable fill）：补前/后倒 V 缺口，底边贴坡线 z=6.968，顶到脊
GZ = 6.968
for tag, gy0, gy1 in (("Front", 0.0, WT), ("Back", 8.0 - WT, 8.0)):
    mesh_box(f"GableBand_{tag}", (0.0, gy0, 6.72), (10.0, gy1, GZ), "mat-0")
    tp = [[0.0, gy0, GZ], [10.0, gy0, GZ], [5.0, gy0, RB + RH],
          [0.0, gy1, GZ], [10.0, gy1, GZ], [5.0, gy1, RB + RH]]
    ti = [0, 1, 2, 3, 5, 4, 0, 2, 5, 0, 5, 3, 2, 1, 4, 2, 4, 5, 0, 3, 4, 0, 4, 1]
    mesh(f"Gable_{tag}", tp, ti, "mat-0")

# ============================== 老虎窗 ×2 ==============================
for i, cx in enumerate((2.65, 7.35)):
    z_base = 7.28
    mesh_box(f"Dormer{i}", (cx - 0.62, 1.05, z_base), (cx + 0.62, 2.55, z_base + 1.30), "mat-0")
    # 迷你双坡顶（脊沿 Y）
    d_r, d_h = z_base + 1.30, 0.72
    dp = [[cx - 0.78, 0.92, d_r], [cx + 0.78, 0.92, d_r], [cx, 0.92, d_r + d_h],
          [cx - 0.78, 2.68, d_r], [cx + 0.78, 2.68, d_r], [cx, 2.68, d_r + d_h]]
    di = [0, 1, 2, 3, 5, 4, 0, 2, 5, 0, 5, 3, 2, 1, 4, 2, 4, 5, 0, 3, 4, 0, 4, 1]
    mesh(f"DormerRoof{i}", dp, di, "mat-1")
    furniture(f"老虎窗框{i}", "window", (cx - 0.55, 0.98, z_base + 0.12), [1.10, 0.16, 0.95], 0.0, "mat-2")
    mesh_box(f"DormerGlass{i}", (cx - 0.48, 1.075, z_base + 0.17), (cx + 0.48, 1.115, z_base + 1.02), "mat-4")

# ============================== 烟囱 ==============================
mesh_box("Chimney", (8.55, 3.55, 6.50), (9.45, 4.45, 10.35), "mat-0", "xz", 1.6)
mesh_box("ChimneyCap", (8.40, 3.40, 10.35), (9.60, 4.60, 10.56), "mat-5")

# ============================== 相机与交付物 ==============================
# 日光（显式光源：自动补光让位）
[light_id] = alloc(1)
op("light.create", {"name": "Sun", "light_kind": "area", "translation": [-12, -14, 26], "color": [1.0, 0.95, 0.86],
                    "intensity": 12.0, "size": 12.0})

hero_id = camera("HeroCam", [17.5, -13.5, 8.2], [5.0, 3.2, 4.2], 32)
op("render.high_quality", {"path": "D:/Word/arch0-spike/deliverables/villa_hero.png",
                           "width": 1600, "height": 900, "spp": 512, "exposure": 0.5, "renderer": "gpu"})

# 细部机位：移动相机（object.transform）+ 重新对准（camera.aim）
op("object.transform", {"id": hero_id, "translate": [-8.9, 8.3, -5.5]})
op("camera.aim", {"id": hero_id, "target": [5.0, 0.4, 2.9]})
op("render.high_quality", {"path": "D:/Word/arch0-spike/deliverables/villa_detail.png",
                           "width": 1600, "height": 900, "spp": 512, "exposure": 0.5, "renderer": "gpu"})

op("render.walkthrough", {"dir": "D:/Word/arch0-spike/deliverables/villa_walk",
                          "waypoints": [[16, -11, 5.6], [20, 2, 5.8], [14, 15, 6.0],
                                        [0, 18, 5.8], [-12, 8, 5.6], [-11, -7, 5.4], [4, -15, 5.6], [16, -11, 5.6]],
                          "target": [5, 3.2, 3.8], "frames": 32, "width": 960, "height": 540,
                          "spp": 32, "exposure": 0.5, "fps": 30, "renderer": "gpu"})

op("render.panorama", {"path": "D:/Word/arch0-spike/deliverables/villa_panorama.png",
                       "width": 4096, "height": 2048, "spp": 96, "exposure": 0.5, "renderer": "gpu"})

op("web.export", {"path": "D:/Word/arch0-spike/deliverables/villa_review.html", "title": "精致二层小洋楼"})

# ---------- 通用格式导出（互操作通道） ----------
op("io.save_scene", {"path": "D:/Word/arch0-spike/deliverables/villa_scene.json"})
op("io.export_gltf", {"path": "D:/Word/arch0-spike/deliverables/villa_model.gltf"})
op("io.export_ifc", {"path": "D:/Word/arch0-spike/deliverables/villa.ifc"})
op("io.export_fbx", {"path": "D:/Word/arch0-spike/deliverables/villa.fbx"})

with open(r"D:\Word\archcore\demo\villa.json", "w", encoding="utf-8") as f:
    json.dump({"ops": ops}, f, ensure_ascii=False, indent=1)
print(f"ops: {len(ops)} | objects tracked: {counter[0]}")

