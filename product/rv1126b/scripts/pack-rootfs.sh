#!/bin/bash
# pack-rootfs.sh — rootfs.tar → ext4 镜像(可直接 dd 到 p7)
# 用 mkfs.ext4 -d 从目录直接灌,免 loop mount(容器内无特权也能跑)
# 用法: pack-rootfs.sh <rootfs.tar> <out.ext4> [大小 MB,默认 512]
set -e
TAR="${1:?tar path}"
OUT="${2:?out ext4}"
SIZE_MB="${3:-512}"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$WORK/root"
tar xf "$TAR" -C "$WORK/root"
mkfs.ext4 -q -L rootfs -d "$WORK/root" "$WORK/rootfs.ext4" "${SIZE_MB}M"
mv "$WORK/rootfs.ext4" "$OUT"
echo "packed: $OUT ($(du -h "$OUT" | cut -f1), ${SIZE_MB}MB fs)"
