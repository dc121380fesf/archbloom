#!/usr/bin/env python3
"""程序化贴图生成器：stucco/屋瓦/草地/木纹 → demo/tex/*.jpg（PPM→ffmpeg，零新依赖）。"""
import math, os, subprocess, struct

OUT = os.path.join(os.path.dirname(__file__), "tex")
os.makedirs(OUT, exist_ok=True)
SIZE = 512


def lcg(seed):
    s = [seed & 0xFFFFFFFF]

    def nxt():
        s[0] = (s[0] * 1664525 + 1013904223) & 0xFFFFFFFF
        return s[0] / 4294967296.0
    return nxt


def value_noise(x, y, freq, seed):
    """网格值噪声（双线性）"""
    fx, fy = x * freq, y * freq
    ix, iy = int(math.floor(fx)), int(math.floor(fy))
    tx, ty = fx - ix, fy - iy
    tx = tx * tx * (3 - 2 * tx)
    ty = ty * ty * (3 - 2 * ty)
    def h(a, b):
        n = (a * 374761393 + b * 668265263 + seed * 1442695041) & 0xFFFFFFFF
        n = (n ^ (n >> 13)) * 1274126177 & 0xFFFFFFFF
        return ((n ^ (n >> 16)) & 0xFFFF) / 65536.0
    v00, v10 = h(ix, iy), h(ix + 1, iy)
    v01, v11 = h(ix, iy + 1), h(ix + 1, iy + 1)
    return (v00 * (1 - tx) + v10 * tx) * (1 - ty) + (v01 * (1 - tx) + v11 * tx) * ty


def fbm(x, y, seed, octaves=4, freq=8.0):
    v, amp, f = 0.0, 0.5, freq
    for _ in range(octaves):
        v += amp * value_noise(x, y, f, seed)
        amp *= 0.5
        f *= 2.0
    return v


def write_jpg(name, pixel_fn):
    path = os.path.join(OUT, name + ".ppm")
    with open(path, "wb") as f:
        f.write(b"P6\n%d %d\n255\n" % (SIZE, SIZE))
        buf = bytearray()
        for y in range(SIZE):
            for x in range(SIZE):
                r, g, b = pixel_fn(x / SIZE, y / SIZE)
                buf += bytes((
                    max(0, min(255, int(r * 255))),
                    max(0, min(255, int(g * 255))),
                    max(0, min(255, int(b * 255)))))
        f.write(buf)
    dst = os.path.join(OUT, name + ".jpg")
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", path, "-q:v", "2", dst], check=True)
    os.remove(path)
    print("tex:", dst)


# 灰泥：暖白底 + 细噪 + 大尺度斑驳
def stucco(u, v):
    n1 = fbm(u, v, 11, 4, 24.0)
    n2 = fbm(u, v, 77, 2, 3.0)
    k = 0.93 + (n1 - 0.5) * 0.10 + (n2 - 0.5) * 0.10
    return (k, k * 0.965, k * 0.90)


# 屋瓦：陶土底 + 横向瓦行 + 逐瓦色差 + 勾缝阴影
def roof_tiles(u, v):
    rows, cols = 10.0, 22.0
    rv, cu = v * rows, u * cols
    row = int(rv)
    off = 0.5 if row % 2 else 0.0
    tile = int(cu + off)
    th = rv - row          # 行内纵向
    tc = (cu + off) - tile  # 瓦内横向
    shade = (math.sin(th * math.pi) * 0.22 + 0.82)          # 瓦拱形明暗
    joint = 1.0 if (tc > 0.045 and tc < 0.955 and th > 0.07) else 0.62  # 勾缝
    vary = value_noise(tile * 0.37, row * 0.53, 3.0, 9) * 0.22 + 0.89
    k = shade * joint * vary
    grain = (fbm(u, v, 5, 3, 40.0) - 0.5) * 0.10
    return ((0.62 + grain) * k, (0.235 + grain * 0.5) * k, (0.125 + grain * 0.35) * k)


# 草地：密噪 + 斑块 + 偏黄绿变化
def grass(u, v):
    n1 = fbm(u, v, 21, 4, 60.0)
    n2 = fbm(u, v, 5, 2, 6.0)
    k = 0.75 + (n1 - 0.5) * 0.5 + (n2 - 0.5) * 0.35
    return (0.20 * k + 0.06, 0.34 * k + 0.10, 0.13 * k + 0.03)


# 木纹：纵向纹理 + 年轮扰动
def wood(u, v):
    warp = fbm(u, v, 33, 2, 5.0)
    rings = math.sin((u * 14.0 + warp * 3.2 + math.sin(v * 2.2) * 0.8) * math.pi) * 0.5 + 0.5
    k = 0.62 + rings * 0.30 + (fbm(u, v, 3, 3, 30.0) - 0.5) * 0.12
    return (0.44 * k, 0.27 * k, 0.155 * k)


# 石材：勒脚/台阶
def stone(u, v):
    block_u, block_v = u * 6.0, v * 3.0
    bu, bv = int(block_u), int(block_v)
    joint = 1.0 if (block_u - bu > 0.04 and block_v - bv > 0.09) else 0.66
    vary = value_noise(bu * 1.7, bv * 2.3, 2.0, 44) * 0.22 + 0.88
    k = joint * vary * (0.92 + (fbm(u, v, 8, 3, 20.0) - 0.5) * 0.16)
    return (0.58 * k, 0.55 * k, 0.505 * k)


write_jpg("stucco_wall", stucco)
write_jpg("roof_tiles", roof_tiles)
write_jpg("grass", grass)
write_jpg("wood_floor", wood)
write_jpg("stone_block", stone)
print("done")
