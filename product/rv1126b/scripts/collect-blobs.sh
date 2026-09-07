#!/bin/bash
# collect-blobs.sh — 从 SDK 收集 Rockchip 二进制到 overlay
# 用法: collect-blobs.sh <Aura-sdk 路径> [overlay 路径]
set -e
SDK="${1:?usage: collect-blobs.sh <Aura-sdk path> [overlay]}"
OVERLAY="${2:-$(dirname "$0")/../board/rv1126b/overlay}"
OVERLAY="$(cd "$OVERLAY" && pwd)"

KO_SRC="$SDK/sysdrv/drv_ko/out"
MPP_SRC="$SDK/media/mpp/out"
RKAIQ_SRC="$SDK/media/isp/out"
RGA_SRC="$SDK/media/rga/out"

mkdir -p "$OVERLAY/usr/ko" "$OVERLAY/usr/lib" "$OVERLAY/usr/bin" "$OVERLAY/etc/iqfiles"

echo "== ko =="
for ko in kmpp.ko kmpp_smart.ko; do
  f=$(find "$KO_SRC" "$SDK/sysdrv" -name "$ko" 2>/dev/null | head -1)
  [ -n "$f" ] && cp -v "$f" "$OVERLAY/usr/ko/" || echo "  !! $ko 未找到"
done

echo "== mpp/rga 库 =="
# 先清旧文件(避免上次的悬空 soname 链接挡住 cp -L)
rm -f "$OVERLAY/usr/lib/"librockchip_mpp.so* "$OVERLAY/usr/lib/"librga.so*
for lib in librockchip_mpp.so librockchip_mpp.so.0 librockchip_mpp.so.1 librga.so librga.so.2; do
  f=$(find -L "$MPP_SRC" "$RGA_SRC" -name "$lib*" 2>/dev/null | head -1)
  # cp -L 解引用,SDK 里一堆 soname 软链
  [ -n "$f" ] && cp -L -v "$f" "$OVERLAY/usr/lib/"
done
# soname 链接
ln -sf librockchip_mpp.so.1 "$OVERLAY/usr/lib/librockchip_mpp.so"
ln -sf librockchip_mpp.so.0 "$OVERLAY/usr/lib/librockchip_mpp.so.1"
ln -sf librga.so.2 "$OVERLAY/usr/lib/librga.so"

echo "== rkaiq =="
f=$(find "$RKAIQ_SRC" -name "rkaiq_3A_server" -type f 2>/dev/null | head -1)
[ -n "$f" ] && cp -v "$f" "$OVERLAY/usr/bin/" || echo "  !! rkaiq_3A_server 未找到"
for lib in librkaiq.so; do
  f=$(find "$RKAIQ_SRC" -name "$lib*" 2>/dev/null | head -1)
  [ -n "$f" ] && cp -v "$f" "$OVERLAY/usr/lib/"
done

echo "== C++ 运行时(buildroot 不装 C++ 包就不带这些库,rkaiq/live777 都要) =="
# libstdc++ 必须和 live777 的构建链一致(crossbuilder-rkmpp,gcc14 时代,
# CXXABI_1.3.15);从 rkmpp cross 镜像/usr/aarch64-linux-gnu/lib 取。
# libgcc_s 从 buildroot 工具链 lib64 取即可。
TC_LIB64="$OUT/host/aarch64-buildroot-linux-gnu/lib64"
[ -d "$TC_LIB64" ] || TC_LIB64=$(find "$OUT/host" -maxdepth 3 -name lib64 -type d 2>/dev/null | head -1)
for lib in libgcc_s.so.1; do
  f=$(find "$TC_LIB64" -name "$lib" 2>/dev/null | head -1)
  [ -n "$f" ] && cp -v "$f" "$OVERLAY/usr/lib/"
done
RKMPP_IMG_LIB="${RKMPP_IMG_LIB:-$(dirname "$0")/../blobs}"
if [ -f "$RKMPP_IMG_LIB/libstdc++.so.6.0.33" ]; then
  cp -v "$RKMPP_IMG_LIB/libstdc++.so.6.0.33" "$OVERLAY/usr/lib/"
  ln -sf libstdc++.so.6.0.33 "$OVERLAY/usr/lib/libstdc++.so.6"
else
  echo "  !! $RKMPP_IMG_LIB/libstdc++.so.6.0.33 未找到(从 crossbuilder-aarch64-rkmpp 镜像提取一次放过去)"
fi

echo "== iqfiles =="
# 板子是 ISP HW ver 35:必须选 isp35 的 IQ。裸 find | head -1 会先撞见
# isp33 目录拿错版本(isp33 IQ 在 isp35 硬件上 AE 配置会 fatal)。
IQ=$(find "$RKAIQ_SRC" -path "*isp35/common/sc450ai_default_default.json" 2>/dev/null | head -1)
[ -z "$IQ" ] && IQ=$(find "$RKAIQ_SRC" -path "*isp35*" -name "sc450ai_default_default.json" 2>/dev/null | head -1)
[ -z "$IQ" ] && IQ=$(find "$RKAIQ_SRC" -name "sc450ai_default_default.json" 2>/dev/null | head -1)
if [ -n "$IQ" ]; then
  cp -v "$IQ" "$OVERLAY/etc/iqfiles/"
  # AINR 模型目录(如果有)
  SRC_DIR=$(dirname "$IQ")/sc450ai
  [ -d "$SRC_DIR" ] && cp -r "$SRC_DIR" "$OVERLAY/etc/iqfiles/"
  python3 "$(dirname "$0")/apply-iq-patch.py" "$OVERLAY/etc/iqfiles/sc450ai_default_default.json"
else
  echo "  !! sc450ai iqfile 未找到"
fi

echo "== done =="
