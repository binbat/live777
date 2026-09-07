#!/usr/bin/env python3
"""IQ json 补丁:FPV 120fps 确定性配置(2026-09-08 验证定稿)。

主补丁:AE route 曝光时间帽 1/240s(快门优先)
- linAeCtrl.route.sw_aeT_time_dot 全部 clamp 到 0.00417s
- 曝光焊死 ~390 行,暗处由增益补偿:延时和动态清晰度优先于亮度
- 该帽同时解除 AE 用 vblank 延展/换模式降帧的动机(120fps 自然焊死)

可选防御(当前验证基线未启用,需要时取消注释):
- sensor_calib.CISMinFps = 60   (120fps 模式模型帧率为 60,进一步挡 vblank 延展)
- ae frmRate = fix 60           (锁死模型帧率;合法枚举 ae_frmRate_fix_mode,
                                 manual_mode 是瞎编的会被 rkaiq 拒并回落 auto)
- aibnr frmRate 10->60          (AI 降噪帧率假设;120fps 下吃 CPU,谨慎开)

用法: apply-iq-patch.py <iq.json> (原地修改)
"""
import json
import sys

EXPOSURE_CAP_S = 0.00417  # 1/240s;120fps 下 ~390 行

p = sys.argv[1]
d = json.load(open(p))

n = 0
for ms in d.get("main_scene", []):
    for ss in ms.get("sub_scene", []):
        route = (
            ss.get("scene_isp35", {})
            .get("ae_calib", {})
            .get("linAeCtrl", {})
            .get("route", {})
        )
        dots = route.get("sw_aeT_time_dot")
        if not dots:
            continue
        new = [min(t, EXPOSURE_CAP_S) for t in dots]
        # route 插值要求单调不减
        for i in range(1, len(new)):
            if new[i] < new[i - 1]:
                new[i] = new[i - 1]
        route["sw_aeT_time_dot"] = new
        n += 1

# 紧凑输出(indent=4 会把 3.3MB 的 IQ 撑到 14MB)
json.dump(d, open(p, "w"), separators=(",", ":"))
print(f"{p}: AE exposure capped at {EXPOSURE_CAP_S}s in {n} scene(s)")
