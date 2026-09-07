#!/bin/bash
# build.sh — RV1126B 产品 buildroot 一键构建
#
# 用法:
#   ./build.sh            全流程:抽树 → defconfig → make → blobs → pack
#   ./build.sh rootfs     只增量 buildroot make
#   ./build.sh pack       只重新收集 blobs + 打 ext4
#
# 环境(在 lilith 的 luckfox-sdk-builder docker 里跑,或任何满足
# buildroot 依赖的 Linux):
#   AURA_SDK   Aura-sdk 路径(默认 ~/rv1126b/Aura-sdk)
#   LIVE777    live777 aarch64 二进制路径(默认 target 产物)
set -e
cd "$(dirname "$0")"
AURA_SDK="${AURA_SDK:-$HOME/rv1126b/Aura-sdk}"
BR_TARBALL="$AURA_SDK/sysdrv/tools/board/buildroot/buildroot-2025.02.6.tar.gz"
BR_TREE="$AURA_SDK/sysdrv/source/buildroot/buildroot-2025.02.6"
OUT="$(pwd)/output"
LIVE777="${LIVE777:-$HOME/live777/target/aarch64-unknown-linux-gnu/release/live777}"

step_extract() {
  if [ ! -d "$BR_TREE" ]; then
    echo "== 抽 buildroot 树 =="
    mkdir -p "$(dirname "$BR_TREE")"
    tar xzf "$BR_TARBALL" -C "$(dirname "$BR_TREE")"
  fi
}

step_defconfig() {
  echo "== defconfig =="
  make -C "$BR_TREE" O="$OUT" BR2_EXTERNAL="$(pwd)" binbat_rv1126b_defconfig
}

step_make() {
  echo "== buildroot make =="
  # 容器内 root 构建:host-tar 等的 configure 拒绝 root,官方豁免开关
  export FORCE_UNSAFE_CONFIGURE=1
  make -C "$OUT" -j"$(nproc)"
}

step_blobs() {
  echo "== 收集 blobs =="
  scripts/collect-blobs.sh "$AURA_SDK" "$(pwd)/board/rv1126b/overlay"
  if [ -f "$LIVE777" ]; then
    cp -v "$LIVE777" "$(pwd)/board/rv1126b/overlay/usr/bin/live777"
    chmod +x "$(pwd)/board/rv1126b/overlay/usr/bin/live777"
  else
    echo "  !! live777 二进制未找到($LIVE777)"
  fi
}

step_pack() {
  echo "== 打 ext4 =="
  scripts/pack-rootfs.sh "$OUT/images/rootfs.tar" "$(pwd)/output/rootfs.ext4" 512
}

case "${1:-all}" in
  all)     step_extract; step_defconfig; step_blobs; step_make; step_pack ;;
  rootfs)  step_extract; step_defconfig; step_make ;;
  pack)    step_make; step_pack ;;
  *)       echo "usage: $0 [all|rootfs|pack]"; exit 1 ;;
esac
