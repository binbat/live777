# patches/ — RV1126B 产品 patch 清单

对第三方基线(Rockchip SDK / 厂商 IQ)的全部产品修改,以 patch 或
patch 脚本形式固化在此。overlay/ 下的文件(init.d、live777.toml 等)
本身就是产品资产,不是 patch。

## kernel/0001-sc450ai-bringup-60-120fps-modes.patch

对 SDK 内核树(`Aura-sdk/sysdrv/source/kernel/`,git)的完整产品 delta,
一条 patch 全含(2026-09-08 内核 #17 验证):

- `rv1126b-evb-cam-csi0.dtsi`:SC450AI 适配(reset/pwdn GPIO、4lane 端点)
- `rv1126b_luckfox_defconfig`:`CONFIG_VIDEO_SC450AI=y`(必须 built-in,
  模块方式 dphy fwnode 解析会跳过未绑 driver 的 sensor)
- `drivers/media/i2c/sc450ai.c`:
  - 60fps 模式 = 30fps 表仅 VTS 减半(链路速率与厂商验证态一致)
  - 120fps 模式 = Rockchip develop-5.10 官方 4lane 表(187 条),
    `hts_def=0x180a` + `link_freq_idx=1`(720Mbps,模型帧率 60fps
    与 vendor "/2" 约定一致,AE 不飞表)

应用:`cd Aura-sdk/sysdrv/source/kernel && git apply <本文件>`,然后
`./build.sh kernel`,刷 boot 分区(mmcblk0p4,可带系统 dd)。

## scripts/apply-iq-patch.py(IQ patch 机制)

IQ json 太大且机器生成,静态 patch 不可维护,用脚本原地改:

1. `collect-blobs.sh` 从 SDK 选 **isp35** 的 `sc450ai_default_default.json`
   (板子是 ISP HW ver 35;裸 `find|head -1` 会错拿 isp33——已修)
2. `apply-iq-patch.py` 打 **AE route 曝光帽 1/240s**(linAeCtrl.route
   的 `sw_aeT_time_dot` clamp 到 0.00417s):快门优先,曝光 ~390 行,
   暗处增益补,120fps 自然焊死。紧凑 JSON 输出(indent 会把 3.3MB
   撑到 14MB)

## 关联的非 patch 资产(overlay/)

- `etc/live777.toml`:1344x752@120fps H.265 CBR 2.5M(752 是因 760 非
  16 对齐、编码器 pad 出绿线;ISP 裁底 8 行)
- `etc/init.d/S90live777`:启动前等全局 inet(ICE 候选启动时一次采集,
  赶在 dhcpcd 前启动会只拿到 127.0.0.1 → WHEP 全灭)
- `etc/init.d/S95watchdog`:bitrate 取 publish 侧(tr 切分后第一个,
  贪婪 sed 会误取 subscribe 聚合=无人观看恒 0 → 全天误杀);重启委托
  `/etc/init.d/S90live777 start`(单一入口,继承等待网络等逻辑)
- `etc/init.d/S50rkaiq`:rkaiq_3A_server 启动(注意 rkaiq 死等
  STREAM_START 事件,流已在跑时启动它会导致 ISP 裸奔灰白——
  rkaiq_3A_server.cpp 源码修复待做)
