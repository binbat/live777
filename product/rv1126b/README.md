# RV1126B 产品固件(buildroot)

面向 FPV 摄像头产品的 buildroot 构建树。与厂商 SDK 的关系:**SDK 只提供
buildroot 源码树、内核/uboot 构建和 Rockchip 二进制(ko/mpp/rkaiq)**,
产品侧 defconfig/overlay/打包全部在本目录,不侵入 SDK。

当前产品基线(2026-09-08):**1344x752@120fps H.265 CBR 2.5M,rkaiq 在线
(快门优先 1/240s),glass-to-glass 实测 ~80ms。**

## 目录

```
configs/binbat_rv1126b_defconfig   # 产品 buildroot 配置(极简)
board/rv1126b/overlay/             # rootfs overlay(init 脚本/配置/二进制)
patches/kernel/                    # SDK 内核树的产品 patch(git apply 复现)
patches/README.md                  # patch 清单与机制说明
scripts/collect-blobs.sh           # 从 SDK 收集 ko/mpp/rkaiq/iqfiles(isp35)→ overlay
scripts/apply-iq-patch.py          # IQ 补丁(AE route 曝光帽 1/240s)
scripts/pack-rootfs.sh             # rootfs.tar → ext4 镜像
build.sh                           # 一键构建
```

## 前置

- lilith `~/rv1126b/Aura-sdk`:内核侧改动以 patch 形式保存在
  `patches/kernel/`,`git apply` 后 `./build.sh kernel` 产出 boot.img
- 构建环境:luckfox-sdk-builder docker(ubuntu 22.04)
- live777 二进制:Mac 交叉构建(crossbuilder-aarch64-rkmpp,native-rkmpp,webui)
- blobs/libstdc++.so.6.0.33:从 crossbuilder-aarch64-rkmpp 镜像提取
  (live777 构建链 gcc14 需要 CXXABI_1.3.15)

## 流程

```bash
./build.sh                 # 全部:抽树 → defconfig → make → blobs → ext4
./build.sh rootfs          # 只跑 buildroot make(增量)
./build.sh pack            # 只重打 ext4(overlay 改了之后)
```

产物:`output/rootfs.ext4`(刷到 p7),内核/启动链走 SDK output/image/。

## 刷写

**rootfs 必须走 maskrom**(`db` 进 maskrom 后 `wl <rootfs-sector> rootfs.ext4`,
扇区见 parameter 表)。**绝对不要系统内 dd p7**——往运行中的根分区直接写
会把系统带崩(已翻车一次)。boot 分区(p4)可以系统内 dd 后 reboot。
