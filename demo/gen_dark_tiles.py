#!/usr/bin/env python3
"""生成新中式青瓦（黛瓦）贴图：深蓝灰筒瓦行列 + 压瓦缝，逐瓦随机明度微差。"""
from PIL import Image
import random

random.seed(42)
S = 512
img = Image.new("RGB", (S, S))
px = img.load()

ROWS, COLS = 10, 16
base = (66, 74, 88)      # 青灰基调
for r in range(ROWS):
    y0 = r * S // ROWS
    y1 = (r + 1) * S // ROWS
    for c in range(COLS):
        x0 = c * S // COLS
        x1 = (c + 1) * S // COLS
        v = random.uniform(0.88, 1.12)
        col = tuple(min(255, max(0, int(ch * v))) for ch in base)
        for y in range(y0, y1):
            for x in range(x0, x1):
                # 瓦缝：行间 + 列间的深色线
                edge = (y - y0 < 3) or (y1 - y - 1 < 3) or (x - x0 < 2) or (x1 - x - 1 < 2)
                if edge:
                    px[x, y] = (col[0] // 3, col[1] // 3, col[2] // 3)
                else:
                    # 筒瓦的纵向明暗（圆弧感）
                    t = (x - x0) / max(1, x1 - x0)
                    shade = 0.82 + 0.36 * (1.0 - abs(2 * t - 1) ** 1.6)
                    px[x, y] = tuple(min(255, int(ch * shade)) for ch in col)

img.save(r"D:\Word\archcore\demo\tex\dark_tiles.jpg", quality=92)
print("dark_tiles.jpg written")
