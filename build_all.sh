#!/bin/bash
set -e

echo "======================================="
echo "   Live777 Dual-Platform Builder"
echo "======================================="

# 1. 编译 Raspberry Pi 版本
echo ">>> Building for Raspberry Pi (live777_pi)..."
unset CXXFLAGS
cargo clean -p liveion # 确保底层 C++ 构建宏被清理
cargo build --release -j 2 --features source-libcamera,webui
cp target/release/live777 live777_pi
echo "[OK] Pi Output is ready: ./live777_pi"

echo "---------------------------------------"

# 2. 编译 RDK X5 版本
echo ">>> Building for D-Robotics RDK X5 (live777_rdk)..."
export CXXFLAGS="-DPLATFORM_RDK=ON"
cargo clean -p liveion # 确保底层 C++ 构建宏被重新触发
cargo build --release -j 2 --features source-libcamera,webui
cp target/release/live777 live777_rdk
echo "[OK] RDK Output is ready: ./live777_rdk"

echo "======================================="
echo "Done! Please use live777_pi for Raspberry Pi and live777_rdk for RDK X5."
