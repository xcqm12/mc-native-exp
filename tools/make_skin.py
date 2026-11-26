# -*- coding: utf-8 -*-
"""生成「小七」AI 伙伴的 128×128 双层皮肤（Slim 3px 臂，纹素级精绘）。

设计参照：哥特萝莉——黑长直发、白帽黑丝带、白衬衫 + 黑束腰（白 Z 字）、
黑白多层长裙 + 毛绒裙缘、灰白狐尾、深色条纹袜 + 白靴。
base + overlay（帽/衣/袖/裤）双层全用；布局 = 64×64 Slim 模板 ×2。
"""
from PIL import Image

S = 128
img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw = img
d = __import__("PIL.ImageDraw", fromlist=["ImageDraw"]).Draw(img)

# ── 调色板 ───────────────────────────────────────────────────
HAIR1 = (24, 24, 36, 255)
HAIR2 = (36, 36, 54, 255)
HAIR3 = (52, 52, 78, 255)
HAIR4 = (74, 74, 108, 255)
SKIN = (246, 228, 214, 255)
SKIN2 = (232, 206, 190, 255)
BLUSH = (242, 198, 192, 255)
EYE_D = (36, 30, 56, 255)
EYE_M = (92, 74, 132, 255)
EYE_L = (156, 136, 196, 255)
WHITE = (246, 244, 238, 255)
WHITE2 = (222, 219, 210, 255)
WHITE3 = (198, 194, 184, 255)
BLK = (30, 30, 42, 255)
BLK2 = (46, 46, 62, 255)
COR_L = (60, 60, 78, 255)
STK = (40, 40, 58, 255)
STK_L = (58, 58, 82, 255)
FUR1 = (232, 230, 222, 255)
FUR2 = (202, 200, 192, 255)
FUR3 = (164, 164, 158, 255)
FUR4 = (126, 126, 122, 255)
BOOT = (242, 240, 234, 255)
BOOT2 = (214, 212, 204, 255)
SOLE = (58, 56, 64, 255)
LACE = (250, 248, 244, 255)

def R(x0, y0, x1, y1, c):
    """闭区间矩形（含端点）。"""
    d.rectangle([x0, y0, x1, y1], fill=c)

def P(x, y, c):
    d.point([x, y], fill=c)

def hstrand(x0, x1, y0, y1, seed=0):
    for x in range(x0, x1 + 1):
        for y in range(y0, y1 + 1):
            k = (x * 7 + y * 3 + seed) % 11
            P(x, y, HAIR4 if k == 0 else (HAIR3 if k in (3, 6) else (HAIR2 if k in (1, 4, 8) else HAIR1)))

def fur(x0, x1, y0, y1, seed=0):
    for x in range(x0, x1 + 1):
        for y in range(y0, y1 + 1):
            k = (x * 5 + y * 11 + seed) % 13
            P(x, y, FUR1 if k in (0, 5) else (FUR2 if k in (2, 7, 9) else (FUR3 if k in (3, 10) else FUR4)))

# ══════════════════ 头 base（正面 16,16-32,32）══════════════════
# 全头黑发打底
R(0, 16, 63, 47, HAIR1)
hstrand(0, 15, 16, 47, seed=2)            # 右侧面发丝
hstrand(32, 47, 16, 47, seed=3)           # 左侧面发丝
hstrand(48, 63, 16, 47, seed=4)           # 背面长披发
# 正面脸（16,16-32,32：帽下行 16-21 也画发）
R(18, 16, 29, 21, HAIR2)                  # 刘海（帽下）
for x in range(18, 30):                   # 姬发式刘海锯齿底缘
    if x % 3 == 0:
        P(x, 21, HAIR1)
    else:
        P(x, 21, SKIN)
R(18, 22, 29, 31, SKIN)                   # 脸
R(16, 16, 17, 47, HAIR2); R(30, 16, 31, 47, HAIR2)   # 侧发柱
P(16, 24, HAIR4); P(16, 28, HAIR3); P(31, 26, HAIR4)
# 眼睛（行 26-29；左 19-21，右 26-28）
R(19, 26, 21, 29, EYE_D); R(26, 26, 28, 29, EYE_D)
R(20, 27, 21, 28, EYE_M); R(26, 27, 27, 28, EYE_M)
P(20, 26, EYE_L); P(27, 26, EYE_L)
P(20, 29, EYE_M); P(27, 29, EYE_M)                  # 下虹膜渐变
R(19, 25, 21, 25, BLK); R(26, 25, 28, 25, BLK)      # 上睫线
P(19, 30, BLK); P(28, 30, BLK)                      # 下睫点
# 腮红 + 嘴
P(18, 30, BLUSH); P(29, 30, BLUSH)
R(23, 30, 24, 30, SKIN2)

# ══════════════════ 帽 overlay ══════════════════
# front (80,16)-(96,32)：白帽行 16-21 + 黑丝带行 22-24 + 蕾丝行 25
R(80, 16, 95, 21, WHITE)
R(80, 20, 95, 21, WHITE2)
R(80, 22, 95, 24, BLK)
R(80, 22, 95, 22, BLK2)
for x in range(82, 94, 4):
    P(x, 23, BLK2)
R(80, 25, 95, 25, LACE)
R(80, 16, 81, 25, WHITE2); R(94, 16, 95, 25, WHITE2)
# top (80,0)-(96,16)：白帽顶 + 黑缎带十字
R(80, 0, 95, 15, WHITE)
R(80, 6, 95, 8, BLK); R(86, 0, 88, 15, BLK)
R(80, 14, 95, 15, WHITE2)
# sides：行 16-24 白帽 + 丝带
R(64, 16, 79, 24, WHITE); R(64, 20, 79, 21, WHITE2); R(64, 22, 79, 24, BLK)
R(96, 16, 111, 24, WHITE); R(96, 20, 111, 21, WHITE2); R(96, 22, 111, 24, BLK)
# back (112,16)-(128,32)：白帽行 16-21 + 大黑蝴蝶结 + 垂尾
R(112, 16, 127, 21, WHITE)
R(117, 18, 122, 23, BLK)
R(114, 19, 116, 22, BLK2); R(123, 19, 125, 22, BLK2)
R(119, 24, 120, 31, BLK); P(121, 26, BLK); P(118, 28, BLK)

# ══════════════════ 身体 base（front 40,40-56,64）════════════
R(40, 40, 55, 63, WHITE)
# 竖向压褶（衬衫两翼）
for lx in (41, 43, 52, 54):
    for y in range(42, 56, 2):
        P(lx, y, WHITE2)
# 黑束腰（x 46-54，行 44-56）+ 白镶边 + 系绳
R(46, 44, 54, 56, BLK)
for x in range(48, 54):
    for y in range(46, 55, 2):
        P(x, y, COR_L)
R(46, 44, 46, 56, WHITE3); R(54, 44, 54, 56, WHITE3); R(46, 55, 54, 55, WHITE)
for y in range(46, 54, 2):
    P(50, y, COR_L)
# 白色 Z 字（束腰中央 5×7）
zx, zy = 48, 46
R(zx, zy, zx + 4, zy, WHITE)
P(zx + 4, zy + 1, WHITE); P(zx + 3, zy + 2, WHITE)
P(zx + 2, zy + 3, WHITE); P(zx + 1, zy + 4 - 1, WHITE)
R(zx, zy + 4, zx + 4, zy + 4, WHITE)
# 颈口黑蝴蝶结
R(46, 40, 49, 42, BLK); R(47, 42, 48, 42, BLK2)
R(40, 40, 41, 41, LACE); R(54, 40, 55, 41, LACE)
# 腰缘白褶（行 57-58）+ 裙顶黑（行 58-63）
R(40, 57, 55, 58, WHITE)
R(40, 58, 55, 63, BLK)
for x in range(40, 56, 3):
    P(x, 60, WHITE3)
# back (64,40)-(80,64)：束腰背 + 交叉系带
R(64, 40, 79, 63, WHITE)
R(67, 43, 76, 56, BLK)
for y in range(45, 55, 2):
    R(69, y, 71, y, COR_L); R(72, y, 74, y, COR_L)
R(64, 57, 79, 58, WHITE); R(64, 58, 79, 63, BLK)
R(64, 40, 65, 41, LACE); R(78, 40, 79, 41, LACE)
# sides
R(32, 40, 39, 63, WHITE); R(32, 40, 35, 55, BLK); R(32, 57, 39, 63, BLK)
R(56, 40, 63, 63, WHITE); R(60, 40, 63, 55, BLK); R(56, 57, 63, 63, BLK)
# top (40,32)-(56,40)：领口
R(40, 32, 55, 39, WHITE); R(46, 34, 49, 39, SKIN2)
# bottom：裙内暗
R(56, 32, 71, 39, HAIR1)

# ══════════════════ 衣 overlay（jacket）══════════════════
# front (40,72)-(56,96)：肩发缕 + 毛绒披肩
hstrand(40, 44, 72, 84, seed=5)
hstrand(52, 55, 72, 84, seed=6)
fur(40, 44, 85, 95, seed=1)
fur(52, 55, 85, 95, seed=2)
# back (64,72)-(80,96)：黑发披背 + 狐尾
R(64, 72, 79, 91, HAIR1)
hstrand(64, 79, 72, 91, seed=7)
fur(70, 79, 84, 95, seed=3)
# top overlay (40,64)-(56,72)：发顶沿
R(40, 64, 55, 71, HAIR2)

# ══════════════════ 腿 base（裙 + 毛绒缘 + 袜 + 靴）══════════
def leg_base(fx, fy, outer):
    if outer:
        R(fx, fy, fx + 7, fy + 11, BLK)               # 黑外层裙
        for x in range(fx, fx + 8, 3):
            for y in range(fy, fy + 11):
                P(x, y, BLK2)
        R(fx, fy + 10, fx + 7, fy + 10, WHITE)        # 裙缘白条
    else:
        R(fx, fy, fx + 7, fy + 11, WHITE2)            # 白内层裙
        for x in (fx + 1, fx + 3, fx + 5):
            for y in range(fy, fy + 11):
                P(x, y, WHITE3)
        R(fx, fy + 10, fx + 7, fy + 10, BLK)
    fur(fx, fx + 7, fy + 22, fy + 27, seed=outer and 4 or 5)   # 毛绒裙缘
    R(fx, fy + 28, fx + 7, fy + 37, STK)              # 长袜
    R(fx, fy + 30, fx + 7, fy + 30, STK_L); R(fx, fy + 34, fx + 7, fy + 34, STK_L)
    R(fx, fy + 38, fx + 7, fy + 39, BLK)              # 靴口黑缘
    R(fx, fy + 40, fx + 7, fy + 45, BOOT)             # 白靴
    P(fx, fy + 44, BOOT2); P(fx + 7, fy + 44, BOOT2)
    P(fx + 2, fy + 41, (255, 253, 250, 255)); P(fx + 3, fy + 41, (255, 253, 250, 255))
    R(fx, fy + 46, fx + 7, fy + 47, SOLE)

leg_base(8, 40, True)        # right leg
leg_base(40, 104, False)     # left leg

# ══════════════════ 裤 overlay（裙外摆流动层）════════════════
# right pants front (8,72)-(16,96)：黑裙外摆
R(10, 72, 15, 82, BLK)
for y in range(73, 83, 3):
    for x in range(10, 16, 2):
        P(x, y, BLK2)
R(10, 81, 15, 82, WHITE)
# left pants front (8,104)-(16,128)：白裙内摆
R(8, 104, 13, 115, WHITE2)
for y in range(105, 116, 3):
    for x in range(8, 14, 2):
        P(x, y, WHITE3)
R(8, 114, 13, 115, BLK)

# ══════════════════ 手臂 base（slim 6×24）══════════════════
def arm_base(ax, ay, mirror):
    R(ax, ay, ax + 5, ay + 1, WHITE)              # 泡泡袖
    P(ax, ay, WHITE2); P(ax + 5, ay, WHITE2)
    R(ax, ay + 2, ax + 5, ay + 3, BLK)            # 黑丝带臂环
    R(ax, ay + 4, ax + 5, ay + 17, WHITE)         # 合体袖
    for y in range(8, 17, 3):                     # 侧影
        P(ax + (5 if mirror else 0), ay + y, WHITE2)
    R(ax, ay + 20, ax + 5, ay + 22, LACE)         # 蕾丝袖口
    R(ax, ay + 23, ax + 5, ay + 23, SKIN)         # 手

arm_base(88, 40, False)      # right arm
arm_base(72, 104, True)      # left arm

# ══════════════════ 袖 overlay：蕾丝袖口 + 肩部发缕 ═════════
R(88, 92, 93, 95, LACE)               # right sleeve lace cuff
R(104, 124, 109, 127, LACE)           # left sleeve lace cuff
R(88, 72, 89, 76, HAIR2)              # 右袖肩发缕
R(104, 104, 105, 108, HAIR2)          # 左袖肩发缕

# ── 预览合成 ───────────────────────────────────────────────
def crop(x0, y0, x1, y1):
    return img.crop((x0, y0, x1 + 1, y1 + 1))

front = Image.new("RGBA", (64, 128), (0, 0, 0, 0))
front_layers = [
    ((16, 16, 32, 32), 16, 0),      # 头
    ((80, 16, 96, 32), 16, 0),      # 帽
    ((40, 40, 56, 64), 16, 16),     # 身体
    ((40, 72, 56, 96), 16, 16),     # 衣
    ((88, 40, 94, 64), 10, 16),     # 右臂
    ((88, 72, 94, 96), 10, 16),     # 袖
    ((72, 104, 78, 128), 38, 16),   # 左臂
    ((104, 104, 110, 128), 38, 16), # 左袖
    ((8, 40, 16, 64), 16, 40),      # 右腿
    ((8, 72, 16, 96), 16, 40),      # 右裤
    ((40, 104, 48, 128), 24, 40),   # 左腿
    ((8, 104, 16, 128), 24, 40),    # 左裤
]
for (x0, y0, x1, y1), dx, dy in front_layers:
    front.alpha_composite(crop(x0, y0, x1, y1), (dx, dy))

back = Image.new("RGBA", (64, 128), (0, 0, 0, 0))
back_layers = [
    ((48, 16, 64, 32), 16, 0),      # 头 back
    ((112, 16, 128, 32), 16, 0),    # 帽 back（蝴蝶结垂尾）
    ((64, 40, 80, 64), 16, 16),     # 身体 back
    ((64, 72, 80, 96), 16, 16),     # 衣 back（黑发披背 + 狐尾）
    ((100, 40, 108, 64), 38, 16),   # 右臂 back
    ((100, 72, 108, 96), 38, 16),   # 右袖 back
    ((86, 104, 94, 128), 10, 16),   # 左臂 back
    ((86, 72, 94, 96), 10, 16),     # 左袖 back
    ((24, 40, 32, 64), 24, 40),     # 右腿 back
    ((24, 72, 32, 96), 24, 40),     # 右裤 back
    ((48, 104, 56, 128), 16, 40),   # 左腿 back
    ((48, 72, 56, 96), 16, 40),     # 左裤 back
]
for (x0, y0, x1, y1), dx, dy in back_layers:
    back.alpha_composite(crop(x0, y0, x1, y1), (dx, dy))

prev = Image.new("RGBA", (1024, 640), (44, 50, 66, 255))
prev.alpha_composite(front.resize((256, 512), Image.NEAREST), (100, 64))
prev.alpha_composite(back.resize((256, 512), Image.NEAREST), (660, 64))
prev.convert("RGB").save(r"F:\mc-native-exp\brand\skin-preview.png")

img.save(r"F:\mc-native-exp\brand\skin-xiaoqi-128.png")
img.save(r"F:\mc-native-exp\data\skins\xiaoqi-128.png")
img.resize((64, 64), Image.NEAREST).save(
    r"F:\963\mod\companion-exp\src\main\resources\assets\mcne\textures\entity\xiaoqi.png")
print("skin saved (128x128 slim double-layer; 64x64 game texture)")
