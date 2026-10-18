#!/usr/bin/env python3
"""新中式二层别墅生成器：粉墙黛瓦 + 深棕木作 + 庭院（月洞门/照壁/水池），
带完整室内设计（一层：玄关/客厅/餐厅/厨房/长辈房/楼梯；二层：主卧/次卧/家庭厅/书房）。
全部尺寸按毫米级精度给出（单位：米）。输出 demo/xzs_villa.json 由 archbloom-run 执行。
id 经计数器精确追踪（opening.cut 按 id 引墙）。"""
import json
import math

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
    for (a, b) in boxes:
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


def furniture(name, kind, corner, size, rot, material):
    [oid] = alloc(1)
    a = {"kind": kind, "name": name, "position": corner, "size": size, "material": material}
    if rot:
        a["rotate_z"] = rot
    op("furniture.create", a)
    return oid


def put(kind, name, cx, cy, z, sx, sy, sz, rot, material):
    """按视觉包围盒中心 (cx,cy) 摆家具。rot=绕 Z 逆时针（度）。
    柜类（cabinet）正面在局部 +y；sofa/chair/bed/bookshelf 背面在局部 +y。"""
    if rot == 0:
        px, py = cx - sx / 2, cy - sy / 2
    elif rot == 90:
        px, py = cx + sy / 2, cy - sx / 2
    elif rot == 180:
        px, py = cx + sx / 2, cy + sy / 2
    else:
        px, py = cx - sy / 2, cy + sx / 2
    return furniture(name, kind, [round(px, 3), round(py, 3), z], [sx, sy, sz], rot, material)


def cut(wall, kind, c, through, z0, w, h):
    """opening.cut：position = [沿墙中心, 穿墙向墙中值, 洞口底 z]"""
    op("opening.cut", {"wall": wall, "kind": kind,
                       "position": [c, through, z0], "width": w, "height": h})


def camera(name, pos, target, lens=35):
    [oid] = alloc(1)
    op("camera.create", {"name": name, "translation": pos, "target": target, "lens_mm": lens})
    return oid


def light(name, pos, intensity, size, color=(1.0, 1.0, 1.0)):
    [oid] = alloc(1)
    op("light.create", {"name": name, "light_kind": "area", "translation": list(pos),
                        "color": list(color), "intensity": intensity, "size": size})
    return oid


# ============================== 尺寸总表（米） ==============================
# 主宅 12.0 × 9.0 | 墙厚 0.24 | 隔断 0.12
# 台基 0~0.45 | 一层 0.45~3.35 | 腰线 3.35~3.60 | 二层 3.60~6.60 | 檐口 6.60~6.90
# 四坡屋顶：出檐 0.60，檐口 z=6.90，正脊 y=4.5 / x 4.5~7.5 / z=9.50（坡度 27°）
# 庭院围墙 x −3~15 / y −11~9，高 1.75（月洞门段 2.5）
WT = 0.24
F1Z0, F1Z1 = 0.45, 3.35
BAND = (3.35, 3.60)
F2Z0, F2Z1 = 3.60, 6.60
CORN = (6.60, 6.90)
EAVE, RIDGE_Z = 6.90, 9.50

# ============================== 材质 ==============================
mat("mat-wall", "MAT_WhiteWall", [0.92, 0.91, 0.87])                    # 粉墙
mat("mat-tile", "MAT_DarkTile", [0.95, 0.97, 1.02], 0.0, 0.85)          # 黛瓦（贴图自带青灰色）
mat("mat-trim", "MAT_TrimWhite", [0.95, 0.94, 0.90], 0.0, 0.6)          # 白色线脚
mat("mat-wood", "MAT_DarkWood", [0.27, 0.16, 0.10], 0.0, 0.62)          # 深棕木作
mat("mat-wood2", "MAT_WarmWood", [0.46, 0.30, 0.18], 0.0, 0.66)         # 暖木（地板/家具）
mat("mat-stone", "MAT_BlueStone", [0.45, 0.47, 0.49], 0.0, 0.9)         # 青石
mat("mat-stone2", "MAT_Granite", [0.66, 0.65, 0.62], 0.0, 0.88)         # 浅花岗岩
mat("mat-glass", "MAT_Glass", [0.42, 0.50, 0.54], 1.0, 0.15)            # 镜面玻璃
mat("mat-ground", "MAT_Grass", [0.50, 0.58, 0.46], 0.0, 0.95)           # 草地
mat("mat-water", "MAT_Water", [0.10, 0.15, 0.19], 1.0, 0.04)            # 水面（镜面暗色）
mat("mat-fabric", "MAT_Fabric", [0.52, 0.47, 0.41], 0.0, 0.95)          # 织物
mat("mat-bedding", "MAT_Bedding", [0.80, 0.77, 0.70], 0.0, 0.92)        # 床品
mat("mat-accent", "MAT_RedAccent", [0.52, 0.12, 0.10], 0.0, 0.7)        # 中国红
mat("mat-green", "MAT_Bamboo", [0.24, 0.42, 0.22], 0.0, 0.9)            # 竹/植物

for mid, fname in [("mat-wall", "stucco_wall"), ("mat-tile", "dark_tiles"),
                   ("mat-wood", "wood_floor"), ("mat-wood2", "wood_floor"),
                   ("mat-stone", "stone_block"), ("mat-stone2", "stone_block"),
                   ("mat-ground", "grass")]:
    op("material.set_texture", {"material": mid, "slot": "base_color",
                                "uri": f"D:/Word/archcore/demo/tex/{fname}.jpg"})

# ============================== 场地 ==============================
mesh_box("Ground", (-4, -12, -0.30), (16, 9.2, 0.0), "mat-ground", "xy", 6.0)
# 石板甬道：门楼 → 前院 → 门廊
mesh_box("WalkGate", (10.9, -11, 0.0), (12.1, -4.5, 0.06), "mat-stone2", "xy", 2.0)
mesh_box("WalkApron", (3.5, -4.5, 0.0), (12.1, -1.9, 0.06), "mat-stone2", "xy", 2.0)
mesh_box("WalkMoon", (-2.8, -6.1, 0.0), (1.6, -4.9, 0.06), "mat-stone2", "xy", 2.0)
# 台基（勒脚）
mesh_box("Plinth", (-0.16, -0.16, 0.0), (12.16, 9.16, 0.45), "mat-stone", "xy", 3.0)

# ============================== 水池（西院） ==============================
pool_boxes = [((-2.0, -9.8, 0.0), (2.6, -9.55, 0.22)), ((-2.0, -6.45, 0.0), (2.6, -6.2, 0.22)),
              ((-2.0, -9.55, 0.0), (-1.75, -6.45, 0.22)), ((2.35, -9.55, 0.0), (2.6, -6.45, 0.22))]
pp, pi, pu = merge_boxes_uv(pool_boxes, "xy", 1.5)
mesh("PoolRim", pp, pi, "mat-stone", pu)
mesh_box("PoolWater", (-1.75, -9.55, 0.06), (2.35, -6.45, 0.08), "mat-water")
mesh_box("PoolBed", (-1.75, -9.55, 0.0), (2.35, -6.45, 0.05), "mat-stone")

# ============================== 庭院围墙（含月洞门） ==============================
CYH = 1.75          # 围墙高
CAP = 0.12          # 瓦压顶


def wall_cap(name, a, b, z):
    mesh_box(name, (a[0] - 0.06, a[1] - 0.06, z), (b[0] + 0.06, b[1] + 0.06, z + CAP), "mat-tile")


# 南墙（院门洞 x 10.4~12.8）
mesh_box("YardWall_S1", (-3, -11, 0), (10.4, -10.8, CYH), "mat-wall", "xz", 2.5)
mesh_box("YardWall_S2", (12.8, -11, 0), (15, -10.8, CYH), "mat-wall", "xz", 2.5)
wall_cap("YardCap_S1", (-3, -11), (10.4, -10.8), CYH)
wall_cap("YardCap_S2", (12.8, -11), (15, -10.8), CYH)
# 东墙
mesh_box("YardWall_E", (14.8, -11, 0), (15, 9, CYH), "mat-wall", "yz", 2.5)
wall_cap("YardCap_E", (14.8, -11), (15, 9), CYH)
# 西墙：月洞门段（y −7~−4，高 2.5，圆心 z=1.1 / R=1.1）
mesh_box("YardWall_W1", (-3, -11, 0), (-2.8, -7, CYH), "mat-wall", "yz", 2.5)
mesh_box("YardWall_W2", (-3, -4, 0), (-2.8, 9, CYH), "mat-wall", "yz", 2.5)
wall_cap("YardCap_W1", (-3, -11), (-2.8, -7), CYH)
wall_cap("YardCap_W2", (-3, -4), (-2.8, 9), CYH)
# 月洞门：竖条裁切逼近圆洞（条宽 0.1）
moon_boxes = []
R, ZC, MH = 1.1, 1.1, 2.5
y = -7.0
while y < -4.0 - 1e-9:
    dy = (y + 0.05) + 5.5          # 条中心相对圆心 y=−5.5
    y1 = min(y + 0.1, -4.0)
    if abs(dy) >= R:
        moon_boxes.append(((-3, y, 0), (-2.8, y1, MH)))
    else:
        s = math.sqrt(R * R - dy * dy)
        zt, zb = ZC + s, ZC - s
        if zt < MH:
            moon_boxes.append(((-3, y, zt), (-2.8, y1, MH)))
        if zb > 0:
            moon_boxes.append(((-3, y, 0), (-2.8, y1, zb)))
    y = y1
mp, mi, mu = merge_boxes_uv(moon_boxes, "yz", 2.5)
mesh("MoonGateWall", mp, mi, "mat-wall", mu)

# ============================== 门楼（院门） ==============================
mesh_box("GatePillar_W", (9.95, -11.15, 0), (10.4, -10.65, 2.65), "mat-wall")
mesh_box("GatePillar_E", (12.8, -11.15, 0), (13.25, -10.65, 2.65), "mat-wall")
mesh_box("GatePillarCap_W", (9.85, -11.25, 2.65), (10.5, -10.55, 2.82), "mat-tile")
mesh_box("GatePillarCap_E", (12.7, -11.25, 2.65), (13.35, -10.55, 2.82), "mat-tile")
# 门楼小双坡顶（脊沿 x）
gp = [[9.7, -11.75, 2.82], [13.5, -11.75, 2.82], [13.5, -11, 3.35], [9.7, -11, 3.35],
      [9.7, -10.25, 2.82], [13.5, -10.25, 2.82]]
gi = [0, 1, 2, 0, 2, 3, 4, 3, 2, 4, 2, 5, 0, 3, 4, 1, 5, 2]
guv = [[p[0] / 2.0, p[1] / 2.0] for p in gp]
mesh("GateRoof", gp, gi, "mat-tile", guv)
mesh_box("GateRidge", (9.7, -11.08, 3.35), (13.5, -10.92, 3.48), "mat-tile")

# ============================== 照壁 ==============================
mesh_box("ScreenWall", (8.2, -8.7, 0), (10.6, -8.52, 2.30), "mat-wall", "xz", 2.5)
mesh_box("ScreenBase", (8.1, -8.8, 0), (10.7, -8.42, 0.35), "mat-stone")
wall_cap("ScreenCap", (8.2, -8.7), (10.6, -8.52), 2.30)

# ============================== 竹丛与石桌凳 ==============================
for bi, (bx, by) in enumerate([(-2.2, -3.8), (3.2, -9.9), (13.8, -3.4)]):
    mesh_box(f"Planter{bi}", (bx - 0.5, by - 0.5, 0), (bx + 0.5, by + 0.5, 0.45), "mat-stone", "xy", 1.2)
    stalks = []
    for k in range(6):
        ang = k * 1.05 + bi
        sx, sy = bx + 0.32 * math.cos(ang), by + 0.32 * math.sin(ang)
        h = 2.2 + 0.55 * ((k * 7 + bi * 3) % 3) / 2.0
        stalks.append(((sx - 0.05, sy - 0.05, 0.45), (sx + 0.05, sy + 0.05, 0.45 + h)))
    sp, si = merge_boxes(stalks)
    mesh(f"Bamboo{bi}", sp, si, "mat-green")
put("table", "石桌", 1.5, -3.2, 0.0, 1.1, 1.1, 0.72, 0, "mat-stone2")
put("chair", "石凳1", 1.5, -2.35, 0.0, 0.55, 0.55, 0.45, 180, "mat-stone2")
put("chair", "石凳2", 1.5, -4.05, 0.0, 0.55, 0.55, 0.45, 0, "mat-stone2")
put("chair", "石凳3", 0.65, -3.2, 0.0, 0.55, 0.55, 0.45, 270, "mat-stone2")
put("chair", "石凳4", 2.35, -3.2, 0.0, 0.55, 0.55, 0.45, 90, "mat-stone2")

# ============================== 主宅墙体（0.24，两层分段） ==============================
wf1 = mesh_box("WallFront_F1", (0, 0, F1Z0), (12, WT, F1Z1), "mat-wall", "xz", 2.5)
wf2 = mesh_box("WallFront_F2", (0, 0, F2Z0), (12, WT, F2Z1), "mat-wall", "xz", 2.5)
wb1 = mesh_box("WallBack_F1", (0, 9 - WT, F1Z0), (12, 9, F1Z1), "mat-wall", "xz", 2.5)
wb2 = mesh_box("WallBack_F2", (0, 9 - WT, F2Z0), (12, 9, F2Z1), "mat-wall", "xz", 2.5)
wl1 = mesh_box("WallLeft_F1", (0, WT, F1Z0), (WT, 9 - WT, F1Z1), "mat-wall", "yz", 2.5)
wl2 = mesh_box("WallLeft_F2", (0, WT, F2Z0), (WT, 9 - WT, F2Z1), "mat-wall", "yz", 2.5)
wr1 = mesh_box("WallRight_F1", (12 - WT, WT, F1Z0), (12, 9 - WT, F1Z1), "mat-wall", "yz", 2.5)
wr2 = mesh_box("WallRight_F2", (12 - WT, WT, F2Z0), (12, 9 - WT, F2Z1), "mat-wall", "yz", 2.5)

# 楼板（二层结构板 = 一层天花）
mesh_box("Floor2Slab", (WT, WT, BAND[0]), (12 - WT, 9 - WT, BAND[1]), "mat-trim")
# 腰线 + 檐口（白色线脚环）
band_boxes = [((-0.07, -0.07, BAND[0]), (12.07, WT + 0.07, BAND[1])),
              ((-0.07, 9 - WT - 0.07, BAND[0]), (12.07, 9.07, BAND[1])),
              ((-0.07, WT + 0.07, BAND[0]), (WT + 0.07, 9 - WT - 0.07, BAND[1])),
              ((12 - WT - 0.07, WT + 0.07, BAND[0]), (12.07, 9 - WT - 0.07, BAND[1]))]
bp, bi = merge_boxes(band_boxes)
mesh("StringCourse", bp, bi, "mat-trim")
cornice_boxes = [((-0.14, -0.14, CORN[0]), (12.14, 0.38, CORN[1])),
                 ((-0.14, 8.62, CORN[0]), (12.14, 9.14, CORN[1])),
                 ((-0.14, 0.38, CORN[0]), (0.38, 8.62, CORN[1])),
                 ((11.62, 0.38, CORN[0]), (12.14, 8.62, CORN[1]))]
cp, ci = merge_boxes(cornice_boxes)
mesh("Cornice", cp, ci, "mat-trim")
# 角石（四角竖带）
quoin_boxes = []
for qx, qy in ((0, 0), (12, 0), (0, 9), (12, 9)):
    sx, sy = (-1 if qx == 0 else 1), (-1 if qy == 0 else 1)
    quoin_boxes.append(((min(qx, qx + sx * 0.5), min(qy, qy + sy * 0.5), F1Z0),
                        (max(qx, qx + sx * 0.5), max(qy, qy + sy * 0.5), CORN[0])))
qp, qi = merge_boxes(quoin_boxes)
mesh("Quoins", qp, qi, "mat-trim")

# ============================== 门窗开洞 ==============================
# 前墙（y=0.12）：一层大门 + 客厅大窗 + 餐厅窗；二层阳台门 + 主卧窗 + 家庭厅窗
cut(wf1, "door", 6.0, 0.12, F1Z0 + 0.04, 1.6, 2.5)
cut(wf1, "window", 2.7, 0.12, 0.79, 2.6, 2.3)
cut(wf1, "window", 10.1, 0.12, 1.19, 2.2, 1.8)
cut(wf2, "door", 6.0, 0.12, F2Z0 + 0.04, 1.3, 2.3)
cut(wf2, "window", 2.7, 0.12, 4.44, 2.2, 1.7)
cut(wf2, "window", 10.1, 0.12, 4.44, 2.0, 1.7)
# 后墙（y=8.88）
cut(wb1, "window", 2.7, 8.88, 1.19, 1.8, 1.6)
cut(wb1, "window", 10.1, 8.88, 1.29, 1.8, 1.5)
cut(wb2, "window", 2.7, 8.88, 4.49, 1.6, 1.5)
cut(wb2, "window", 10.1, 8.88, 4.49, 1.6, 1.5)
# 左墙（x=0.12）：position = [x 墙中面, y 沿墙中心, z0]
cut(wl1, "window", 0.12, 2.7, 1.19, 1.8, 1.6)
cut(wl1, "window", 0.12, 7.1, 1.29, 1.4, 1.5)
cut(wl2, "window", 0.12, 2.7, 4.49, 1.6, 1.5)
cut(wl2, "window", 0.12, 7.1, 4.49, 1.4, 1.4)
# 右墙（x=11.88）
cut(wr1, "window", 11.88, 2.7, 1.19, 1.8, 1.6)
cut(wr1, "window", 11.88, 7.1, 1.29, 1.4, 1.5)
cut(wr2, "window", 11.88, 2.7, 4.49, 1.6, 1.5)
cut(wr2, "window", 11.88, 7.1, 4.49, 1.4, 1.4)

# ============================== 内隔断（两层） ==============================
pA = mesh_box("Partition_A", (5.40, WT, F1Z0), (5.52, 9 - WT, F1Z1), "mat-wall")
pB = mesh_box("Partition_B", (WT, 5.40, F1Z0), (5.52, 5.52, F1Z1), "mat-wall")
pC = mesh_box("Partition_C", (8.40, 5.40, F1Z0), (12 - WT, 5.52, F1Z1), "mat-wall")
pD = mesh_box("Partition_D", (8.40, WT, F1Z0), (8.52, 9 - WT, F1Z1), "mat-wall")
cut(pA, "door", 5.46, 2.2, 0.49, 1.0, 2.2)      # 客厅门
cut(pA, "door", 5.46, 7.1, 0.49, 1.0, 2.2)      # 长辈房门
cut(pC, "door", 10.1, 5.46, F1Z0, 1.0, 2.2)     # 厨房门
cut(pD, "door", 8.46, 2.2, 0.49, 1.0, 2.2)      # 餐厅门
cut(pD, "door", 8.46, 7.1, 0.49, 1.0, 2.2)      # 厨房门2
qA = mesh_box("Partition2_A", (5.40, WT, F2Z0), (5.52, 9 - WT, F2Z1), "mat-wall")
mesh_box("Partition2_B", (WT, 5.40, F2Z0), (5.52, 5.52, F2Z1), "mat-wall")
qC = mesh_box("Partition2_C", (8.40, 5.40, F2Z0), (12 - WT, 5.52, F2Z1), "mat-wall")
qD = mesh_box("Partition2_D", (8.40, WT, F2Z0), (8.52, 9 - WT, F2Z1), "mat-wall")
cut(qA, "door", 5.46, 2.2, F2Z0 + 0.04, 1.0, 2.2)
cut(qA, "door", 5.46, 7.1, F2Z0 + 0.04, 1.0, 2.2)
cut(qC, "door", 10.1, 5.46, F2Z0 + 0.04, 1.0, 2.2)
cut(qD, "door", 8.46, 2.2, F2Z0 + 0.04, 1.0, 2.2)
cut(qD, "door", 8.46, 7.1, F2Z0 + 0.04, 1.0, 2.2)

# ============================== 室内地面（分材质） ==============================
mesh_box("Floor1_Living", (WT, WT, F1Z0), (5.40, 9 - WT, F1Z0 + 0.04), "mat-wood2", "xy", 1.8)
mesh_box("Floor1_Hall", (5.52, WT, F1Z0), (8.40, 9 - WT, F1Z0 + 0.03), "mat-stone2", "xy", 1.8)
mesh_box("Floor1_Dining", (8.52, WT, F1Z0), (12 - WT, 5.40, F1Z0 + 0.04), "mat-wood2", "xy", 1.8)
mesh_box("Floor1_Kitchen", (8.52, 5.52, F1Z0), (12 - WT, 9 - WT, F1Z0 + 0.03), "mat-stone2", "xy", 1.8)
mesh_box("Floor2_West", (WT, WT, F2Z0), (5.40, 9 - WT, F2Z0 + 0.04), "mat-wood2", "xy", 1.8)
mesh_box("Floor2_Hall", (5.52, WT, F2Z0), (8.40, 9 - WT, F2Z0 + 0.03), "mat-stone2", "xy", 1.8)
mesh_box("Floor2_East", (8.52, WT, F2Z0), (12 - WT, 5.40, F2Z0 + 0.04), "mat-wood2", "xy", 1.8)
mesh_box("Floor2_Study", (8.52, 5.52, F2Z0), (12 - WT, 9 - WT, F2Z0 + 0.03), "mat-stone2", "xy", 1.8)

# ============================== 室内软装（八类家具 · 一层） ==============================
# 客厅（地面 z=0.49）
put("sofa", "主沙发", 1.4, 2.45, 0.49, 0.95, 2.3, 0.82, 90, "mat-fabric")
put("sofa", "次沙发", 2.6, 4.75, 0.49, 1.7, 0.85, 0.82, 0, "mat-fabric")
put("table", "茶几", 2.5, 3.1, 0.49, 1.25, 0.7, 0.45, 0, "mat-wood2")
put("chair", "圈椅1", 4.35, 2.2, 0.49, 0.6, 0.6, 0.95, 270, "mat-fabric")
put("chair", "圈椅2", 4.35, 3.9, 0.49, 0.6, 0.6, 0.95, 270, "mat-fabric")
put("cabinet", "电视柜", 5.15, 2.4, 0.49, 1.8, 0.42, 0.55, 90, "mat-wood")
put("bookshelf", "博古架", 2.7, 5.15, 0.49, 1.6, 0.32, 1.9, 0, "mat-wood")
# 玄关
put("cabinet", "玄关柜", 6.95, 0.85, 0.49, 1.3, 0.4, 0.85, 180, "mat-wood")
# 餐厅
put("table", "餐桌", 10.15, 2.9, 0.49, 1.9, 1.05, 0.76, 0, "mat-wood2")
for i, cx in enumerate((9.5, 10.15, 10.8)):
    put("chair", f"餐椅S{i+1}", cx, 1.95, 0.49, 0.5, 0.5, 0.95, 180, "mat-wood2")
    put("chair", f"餐椅N{i+1}", cx, 3.85, 0.49, 0.5, 0.5, 0.95, 0, "mat-wood2")
put("cabinet", "餐边柜", 11.5, 2.9, 0.49, 1.5, 0.4, 0.9, 90, "mat-wood")
# 厨房（地面 z=0.48）
put("cabinet", "橱柜-北", 10.1, 8.5, 0.48, 2.8, 0.6, 0.92, 180, "mat-wood2")
put("cabinet", "橱柜-东", 11.5, 7.15, 0.48, 2.2, 0.6, 0.92, 90, "mat-wood2")
put("table", "操作台", 9.6, 6.6, 0.48, 1.5, 0.8, 0.9, 0, "mat-stone2")
put("chair", "吧椅1", 9.2, 5.95, 0.48, 0.5, 0.5, 0.95, 180, "mat-wood2")
put("chair", "吧椅2", 9.2, 7.25, 0.48, 0.5, 0.5, 0.95, 0, "mat-wood2")
# 长辈房
put("bed", "长辈床", 1.75, 7.45, 0.49, 1.55, 2.05, 0.85, 0, "mat-bedding")
put("cabinet", "床头柜A", 0.85, 8.45, 0.49, 0.5, 0.45, 0.55, 180, "mat-wood")
put("cabinet", "床头柜B", 2.65, 8.45, 0.49, 0.5, 0.45, 0.55, 180, "mat-wood")
put("cabinet", "衣柜-长辈", 4.95, 7.0, 0.49, 2.2, 0.6, 2.1, 90, "mat-wood")
put("table", "长辈书桌", 3.7, 6.2, 0.49, 1.2, 0.6, 0.75, 0, "mat-wood2")
put("chair", "长辈书椅", 3.7, 5.75, 0.49, 0.55, 0.55, 0.95, 180, "mat-wood2")

# ============================== 室内软装（二层，地面 z=3.64） ==============================
# 主卧
put("bed", "主卧大床", 2.35, 1.65, 3.64, 1.95, 2.15, 0.9, 180, "mat-bedding")
put("cabinet", "床头柜C", 1.15, 0.75, 3.64, 0.5, 0.45, 0.55, 180, "mat-wood")
put("cabinet", "床头柜D", 3.55, 0.75, 3.64, 0.5, 0.45, 0.55, 180, "mat-wood")
put("cabinet", "主卧衣柜", 4.95, 3.4, 3.64, 2.4, 0.6, 2.2, 90, "mat-wood")
put("sofa", "贵妃榻", 1.35, 4.55, 3.64, 0.85, 1.7, 0.75, 90, "mat-fabric")
put("table", "梳妆桌", 3.7, 4.85, 3.64, 1.2, 0.55, 0.75, 0, "mat-wood2")
put("chair", "梳妆椅", 3.7, 4.35, 3.64, 0.55, 0.55, 0.95, 180, "mat-fabric")
# 次卧
put("bed", "次卧床", 1.75, 7.45, 3.64, 1.55, 2.05, 0.85, 0, "mat-bedding")
put("cabinet", "床头柜E", 0.85, 8.45, 3.64, 0.5, 0.45, 0.55, 180, "mat-wood")
put("cabinet", "床头柜F", 2.65, 8.45, 3.64, 0.5, 0.45, 0.55, 180, "mat-wood")
put("cabinet", "次卧衣柜", 4.95, 7.0, 3.64, 2.2, 0.6, 2.1, 90, "mat-wood")
put("table", "次卧书桌", 3.7, 6.2, 3.64, 1.2, 0.6, 0.75, 0, "mat-wood2")
put("chair", "次卧书椅", 3.7, 5.75, 3.64, 0.55, 0.55, 0.95, 180, "mat-wood2")
# 家庭厅
put("sofa", "家庭厅沙发", 10.15, 1.25, 3.64, 2.1, 0.9, 0.82, 180, "mat-fabric")
put("chair", "家庭厅单椅", 11.3, 3.05, 3.64, 0.6, 0.6, 0.95, 270, "mat-fabric")
put("table", "家庭厅茶几", 10.15, 2.5, 3.64, 1.2, 0.7, 0.45, 0, "mat-wood2")
put("cabinet", "影视柜", 11.5, 3.05, 3.64, 1.6, 0.42, 0.55, 90, "mat-wood")
put("bookshelf", "家庭厅书架", 10.15, 4.95, 3.64, 1.5, 0.32, 1.85, 0, "mat-wood")
# 书房
put("table", "画案", 10.1, 6.55, 3.64, 1.7, 0.85, 0.76, 0, "mat-wood2")
put("chair", "官帽椅", 10.1, 5.95, 3.64, 0.6, 0.6, 0.98, 180, "mat-wood")
put("bookshelf", "书架-西", 9.3, 8.45, 3.64, 1.5, 0.32, 1.9, 0, "mat-wood")
put("bookshelf", "书架-东", 10.9, 8.45, 3.64, 1.5, 0.32, 1.9, 0, "mat-wood")
put("cabinet", "茶水柜", 11.5, 7.3, 3.64, 1.2, 0.4, 0.9, 90, "mat-wood")
# 二层过厅
put("cabinet", "端景柜", 6.95, 7.9, 3.64, 1.1, 0.38, 0.85, 0, "mat-wood")

# ============================== 楼梯（一跑，18 步） ==============================
stair_boxes = []
RISE, RUN, NSTEP = (3.64 - 0.49) / 18.0, 0.28, 18
for i in range(NSTEP):
    top = 0.49 + (i + 1) * RISE
    y0 = 8.5 - (i + 1) * RUN
    stair_boxes.append(((6.6, y0, 0.48), (7.9, y0 + RUN, top)))
stp, sti, stu = merge_boxes_uv(stair_boxes, "xz", 1.5)
mesh("Stair", stp, sti, "mat-wood2", stu)
# 楼梯栏杆（扶手 + 立柱）
rail_boxes = [((6.52, 3.36, 1.60), (6.60, 8.52, 1.72))]
for i in range(0, NSTEP, 2):
    top = 0.49 + (i + 1) * RISE
    y0 = 8.5 - (i + 1) * RUN + 0.05
    rail_boxes.append(((6.53, y0, top), (6.59, y0 + 0.10, top + 1.0)))
rp2, ri2 = merge_boxes(rail_boxes)
mesh("StairRail", rp2, ri2, "mat-wood")

# ============================== 门窗框 + 玻璃 + 花格 ==============================
glass, lattice = [], []


def front_window(c, z0, w, h, y_mid, y_out, y_g0, y_g1, mat_frame="mat-wood"):
    """y_mid=墙中面 y；y_out=框外皮 y；y_g0..y_g1=玻璃 y 范围"""
    furniture("窗框", "window", [c - (w + 0.16) / 2, y_out, z0 - 0.06],
              [w + 0.16, 0.38, h + 0.12], 0, mat_frame)
    glass.append(((c - w / 2 + 0.06, y_g0, z0 + 0.06), (c + w / 2 - 0.06, y_g1, z0 + h - 0.06)))
    # 花格：竖梃 + 横档
    nl = max(3, int(w / 0.32))
    for k in range(1, nl):
        x = c - w / 2 + k * w / nl
        lattice.append(((x - 0.022, y_out - 0.02, z0 + 0.04), (x + 0.022, y_out + 0.06, z0 + h - 0.04)))
    for k in range(1, 3):
        z = z0 + k * h / 3
        lattice.append(((c - w / 2 + 0.04, y_out - 0.02, z - 0.022), (c + w / 2 - 0.04, y_out + 0.06, z + 0.022)))


def side_window(cy, z0, w, h, x_mid, x_out, x_g0, x_g1, mat_frame="mat-wood"):
    furniture("窗框", "window", [x_out, cy - (w + 0.16) / 2, z0 - 0.06],
              [0.38, w + 0.16, h + 0.12], 0, mat_frame)
    glass.append(((x_g0, cy - w / 2 + 0.06, z0 + 0.06), (x_g1, cy + w / 2 - 0.06, z0 + h - 0.06)))
    nl = max(3, int(w / 0.32))
    for k in range(1, nl):
        y = cy - w / 2 + k * w / nl
        lattice.append(((x_out - 0.02, y - 0.022, z0 + 0.04), (x_out + 0.06, y + 0.022, z0 + h - 0.04)))
    for k in range(1, 3):
        z = z0 + k * h / 3
        lattice.append(((x_out - 0.02, cy - w / 2 + 0.04, z - 0.022), (x_out + 0.06, cy + w / 2 - 0.04, z + 0.022)))


# 前墙窗（框外皮 y=-0.07，玻璃在墙内 y 0.055~0.185）
for c, z0, w, h in [(2.7, 0.79, 2.6, 2.3), (10.1, 1.19, 2.2, 1.8),
                    (2.7, 4.44, 2.2, 1.7), (10.1, 4.44, 2.0, 1.7)]:
    front_window(c, z0, w, h, 0.12, -0.07, 0.055, 0.185)
# 后墙窗（框外皮 y=9.07）
for c, z0, w, h in [(2.7, 1.19, 1.8, 1.6), (10.1, 1.29, 1.8, 1.5),
                    (2.7, 4.49, 1.6, 1.5), (10.1, 4.49, 1.6, 1.5)]:
    front_window(c, z0, w, h, 8.88, 8.69, 8.815, 8.945)
# 左墙窗（框外皮 x=-0.07）
for cy, z0, w, h in [(2.7, 1.19, 1.8, 1.6), (7.1, 1.29, 1.4, 1.5),
                     (2.7, 4.49, 1.6, 1.5), (7.1, 4.49, 1.4, 1.4)]:
    side_window(cy, z0, w, h, 0.12, -0.07, 0.055, 0.185)
# 右墙窗（框外皮 x=12.07）
for cy, z0, w, h in [(2.7, 1.19, 1.8, 1.6), (7.1, 1.29, 1.4, 1.5),
                     (2.7, 4.49, 1.6, 1.5), (7.1, 4.49, 1.4, 1.4)]:
    side_window(cy, z0, w, h, 11.88, 11.69, 11.815, 11.945)

# 大门（深棕木作门框 + 门扇）与阳台门
furniture("大门", "door", [6.0 - 0.89, -0.07, F1Z0 + 0.04], [1.78, 0.38, 2.64], 0, "mat-wood")
furniture("阳台门", "door", [6.0 - 0.73, -0.07, F2Z0 + 0.04], [1.46, 0.38, 2.44], 0, "mat-wood")
# 匾额 + 灯笼
mesh_box("Plaque", (5.2, -0.10, 3.02), (6.8, -0.02, 3.52), "mat-accent")
lantern_boxes = [((4.02, -1.32, 2.42), (4.28, -1.06, 2.88)), ((7.72, -1.32, 2.42), (7.98, -1.06, 2.88))]
lp, li = merge_boxes(lantern_boxes)
mesh("Lanterns", lp, li, "mat-accent")

gp2, gi2 = merge_boxes(glass)
mesh("Glass", gp2, gi2, "mat-glass")
lp2, li2 = merge_boxes(lattice)
mesh("Lattice", lp2, li2, "mat-wood")

# ============================== 门廊 + 阳台 ==============================
mesh_box("PorchSlab", (3.6, -1.9, 0.30), (8.4, WT, 0.45), "mat-stone2", "xy", 2.0)
mesh_box("Step1", (3.9, -2.85, 0.0), (8.1, -1.9, 0.15), "mat-stone2", "xy", 2.0)
mesh_box("Step2", (4.1, -2.35, 0.15), (7.9, -1.9, 0.30), "mat-stone2", "xy", 2.0)
col_boxes = []
for cx in (4.15, 7.85):
    col_boxes.append(((cx - 0.24, -1.62, 0.45), (cx + 0.24, -1.14, 0.62)))   # 柱础
    col_boxes.append(((cx - 0.14, -1.52, 0.62), (cx + 0.14, -1.24, 3.35)))   # 柱身
    col_boxes.append(((cx - 0.21, -1.59, 3.35), (cx + 0.21, -1.17, 3.52)))   # 柱头
cpo, cpi = merge_boxes(col_boxes)
mesh("PorchColumns", cpo, cpi, "mat-wood")
# 挂落（柱间花格）
frieze = []
for k in range(9):
    x = 4.45 + k * 0.42
    frieze.append(((x - 0.03, -1.42, 2.98), (x + 0.03, -1.34, 3.33)))
frieze.append(((4.35, -1.42, 2.98), (7.65, -1.34, 3.06)))
frieze.append(((4.35, -1.42, 3.25), (7.65, -1.34, 3.33)))
fp, fi = merge_boxes(frieze)
mesh("PorchFrieze", fp, fi, "mat-wood")
# 阳台（门廊之上）
mesh_box("BalconySlab", (3.6, -1.9, BAND[0]), (8.4, WT, 3.80), "mat-trim")
rail_boxes = []


def railing_run(x0, y0, x1, y1):
    dx, dy = x1 - x0, y1 - y0
    ln = math.hypot(dx, dy)
    n = max(2, int(ln / 0.55) + 1)
    for i in range(n):
        t = i / (n - 1)
        cx, cy = x0 + dx * t, y0 + dy * t
        rail_boxes.append(((cx - 0.035, cy - 0.035, 3.80), (cx + 0.035, cy + 0.035, 4.70)))
    rail_boxes.append(((min(x0, x1) - 0.055, min(y0, y1) - 0.055, 4.70),
                       (max(x0, x1) + 0.055, max(y0, y1) + 0.055, 4.84)))
    # 花格横档
    rail_boxes.append(((min(x0, x1) - 0.03, min(y0, y1) - 0.03, 4.20),
                       (max(x0, x1) + 0.03, max(y0, y1) + 0.03, 4.28)))


railing_run(3.6, -1.9, 8.4, -1.9)
railing_run(3.6, -1.9, 3.6, WT)
railing_run(8.4, -1.9, 8.4, WT)
rp, ri = merge_boxes(rail_boxes)
mesh("Balustrade", rp, ri, "mat-wood")

# ============================== 四坡屋顶（正脊 x 4.5~7.5） ==============================
ax0, ax1, ay0, ay1 = -0.6, 12.6, -0.6, 9.6
R1 = (4.5, 4.5, RIDGE_Z)
R2 = (7.5, 4.5, RIDGE_Z)
roof_pos = [[ax0, ay0, EAVE], [ax1, ay0, EAVE], [ax1, ay1, EAVE], [ax0, ay1, EAVE],
            list(R1), list(R2)]
roof_idx = [0, 1, 5, 0, 5, 4,       # 南坡
            3, 4, 5, 3, 5, 2,       # 北坡
            0, 4, 3,                # 西端（hip 三角）
            1, 2, 5]                # 东端
roof_uv = [[p[0] / 3.2, p[1] / 3.2] for p in roof_pos]
mesh("Roof", roof_pos, roof_idx, "mat-tile", roof_uv)
# 吊顶（屋面底，木作）
ceil_pos = [[ax0, ay0, EAVE], [ax1, ay0, EAVE], [ax1, ay1, EAVE], [ax0, ay1, EAVE]]
mesh("RoofCeiling", ceil_pos, [0, 1, 2, 0, 2, 3], "mat-wood", roof_uv[:4])
# 正脊 + 压顶
mesh_box("RidgeCap", (4.3, 4.34, RIDGE_Z), (7.7, 4.66, RIDGE_Z + 0.20), "mat-tile")

# ============================== 灯光（面光源法向恒朝下；面片会以发光板形式入画，取小贴顶） ==============================
light("日光", (-10, -16, 28), 14, 14, (1.0, 0.95, 0.86))
# 吊灯（一层）：客厅/餐厅/厨房/长辈房/玄关
light("吊灯-客厅", (2.7, 2.6, 3.28), 9, 0.8, (1.0, 0.93, 0.82))
light("吊灯-餐厅", (10.15, 2.6, 3.28), 9, 0.8, (1.0, 0.93, 0.82))
light("吊灯-厨房", (10.1, 7.1, 3.28), 9, 0.8, (1.0, 0.95, 0.88))
light("吊灯-长辈房", (2.7, 7.1, 3.28), 9, 0.8, (1.0, 0.93, 0.82))
light("吊灯-玄关", (6.95, 3.0, 3.28), 7, 0.7, (1.0, 0.93, 0.82))
# 吊灯（二层）
light("吊灯-主卧", (2.7, 2.6, 6.40), 10, 0.8, (1.0, 0.93, 0.82))
light("吊灯-次卧", (2.7, 7.1, 6.40), 10, 0.8, (1.0, 0.93, 0.82))
light("吊灯-家庭厅", (10.15, 2.6, 6.40), 10, 0.8, (1.0, 0.93, 0.82))
light("吊灯-书房", (10.1, 7.1, 6.40), 10, 0.8, (1.0, 0.93, 0.82))
light("吊灯-二层过厅", (6.95, 3.0, 6.40), 8, 0.7, (1.0, 0.93, 0.82))
# 窗侧天光板（模拟日光入室，贴顶置于窗上沿内侧）
light("天光-客厅", (2.7, 1.0, 3.28), 6, 1.0, (1.0, 0.97, 0.90))
light("天光-餐厅", (10.1, 1.0, 3.28), 5, 1.0, (1.0, 0.97, 0.90))
light("天光-长辈房", (2.7, 8.2, 3.28), 5, 1.0, (1.0, 0.97, 0.90))
light("天光-厨房", (10.1, 8.2, 3.28), 5, 1.0, (1.0, 0.97, 0.90))
light("天光-主卧", (2.7, 1.0, 6.40), 6, 1.0, (1.0, 0.97, 0.90))
light("天光-家庭厅", (10.1, 1.0, 6.40), 5, 1.0, (1.0, 0.97, 0.90))
light("天光-次卧", (2.7, 8.2, 6.40), 5, 1.0, (1.0, 0.97, 0.90))
light("天光-书房", (10.1, 8.2, 6.40), 5, 1.0, (1.0, 0.97, 0.90))

# ============================== 场景交付（建模脚本止步于此） ==============================
OUT = "D:/Word/deliverables_xzs"
CAM_ID = camera("Cam", [17.5, -14.5, 8.6], [6, 3, 4.2], 30)     # 审阅页默认机位（id 动态追踪）
op("web.export", {"path": f"{OUT}/11_review.html", "title": "新中式二层别墅 · 带室内设计"})
op("io.save_scene", {"path": f"{OUT}/xzs_villa_scene.json"})
op("io.save_archive", {"path": f"{OUT}/xzs_villa.al"})   # .al 自包含存档（场景+贴图单文件）
op("io.export_gltf", {"path": f"{OUT}/xzs_villa.gltf"})
op("io.export_ifc", {"path": f"{OUT}/xzs_villa.ifc"})

with open(r"D:\Word\archcore\demo\xzs_villa.json", "w", encoding="utf-8") as f:
    json.dump({"ops": ops}, f, ensure_ascii=False, indent=1)

# ============================== 渲染脚本（每张一进程，规避 GPU TDR） ==============================
# 现象实录：同进程第二张渲染约 60s 处 nvlddmkm 事件 153（TDR，设备丢失）。
# 对策：io.load_scene 载入成片 → 删默认相机 → 建本机位相机 → 出一张图，全程独立进程。
SHOTS = [
    # (名称, eye, target, lens_mm, w, h, spp)  —— spp 压在 TDR 安全区（>160 的内景/大图会触发 GPU 复位出黑帧）
    ("01_hero",      [17.5, -14.5, 8.6],  [6, 3, 4.2],      30, 1600, 900, 256),
    ("02_courtyard", [12.4, -10.2, 2.4],  [5.2, 0.6, 3.2],  24, 1600, 900, 160),
    ("03_moongate",  [2.8, -6.3, 1.9],    [-3, -5.5, 1.2],  24, 1200, 800, 160),
    ("04_living",    [4.9, 4.5, 1.75],    [1.1, 1.6, 1.35], 20, 1400, 900, 128),
    ("05_dining",    [9.1, 4.7, 1.75],    [11.3, 1.8, 1.2], 20, 1400, 900, 128),
    ("06_master",    [4.2, 4.7, 5.1],     [1.6, 1.2, 4.7],  20, 1400, 900, 128),
    ("07_study",     [8.95, 5.9, 5.1],    [11.0, 7.8, 4.85], 20, 1400, 900, 128),
]
# 全景：等距柱状 2:1（院心）
PANO = ("09_panorama", [6, -5.8, 1.65], [6, 2, 3.5], 24)
# 环场漫游：24 帧圆轨道（帧图后可由 ffmpeg 装配成片）
WALK_N, WALK_R, WALK_C, WALK_Z = 24, 16.5, (6.0, 2.5), 5.9


def shot_ops(name, eye, target, lens, render_args):
    return {"ops": [
        {"op": "io.load_scene", "args": {"path": f"{OUT}/xzs_villa_scene.json"}},
        {"op": "object.delete", "args": {"id": CAM_ID}},
        {"op": "camera.create", "args": {"name": f"cam_{name}", "translation": eye,
                                         "target": target, "lens_mm": lens}},
        {"op": render_args[0], "args": render_args[1]},
    ]}


import os
os.makedirs(r"D:\Word\archcore\demo\xzs_shots", exist_ok=True)
os.makedirs(rf"{OUT}\10_walk", exist_ok=True)
manifest = []

for name, eye, target, lens, w, h, spp in SHOTS:
    expo = 0.9 if name[:2] in ("02", "04", "05", "06", "07") else 0.55   # 内景/院景提亮
    spec = shot_ops(name, eye, target, lens,
                    ("render.high_quality",
                     {"path": f"{OUT}/{name}.png", "width": w, "height": h,
                      "spp": spp, "exposure": expo, "renderer": "gpu"}))
    p = rf"D:\Word\archcore\demo\xzs_shots\{name}.json"
    with open(p, "w", encoding="utf-8") as f:
        json.dump(spec, f, ensure_ascii=False, indent=1)
    manifest.append(p)

name, eye, target, lens = PANO
spec = shot_ops(name, eye, target, lens,
                ("render.panorama",
                 {"path": f"{OUT}/{name}.png", "width": 4096, "height": 2048,
                  "spp": 32, "exposure": 0.55, "renderer": "gpu"}))
p = rf"D:\Word\archcore\demo\xzs_shots\{name}.json"
with open(p, "w", encoding="utf-8") as f:
    json.dump(spec, f, ensure_ascii=False, indent=1)
manifest.append(p)

for i in range(WALK_N):
    a = 2 * math.pi * i / WALK_N
    eye = [WALK_C[0] + WALK_R * math.cos(a), WALK_C[1] + WALK_R * math.sin(a), WALK_Z]
    name = f"10_walk/walk_{i + 1:02d}"
    spec = shot_ops(name, eye, [6, 2.5, 4.2], 26,
                    ("render.high_quality",
                     {"path": f"{OUT}/{name}.png", "width": 960, "height": 540,
                      "spp": 20, "exposure": 0.55, "renderer": "gpu"}))
    p = rf"D:\Word\archcore\demo\xzs_shots\walk_{i + 1:02d}.json"
    with open(p, "w", encoding="utf-8") as f:
        json.dump(spec, f, ensure_ascii=False, indent=1)
    manifest.append(p)

with open(r"D:\Word\archcore\demo\xzs_shots\_manifest.txt", "w", encoding="utf-8") as f:
    f.write("\n".join(manifest))
print(f"ops: {len(ops)} | objects tracked: {counter[0]} | shots: {len(manifest)}")
