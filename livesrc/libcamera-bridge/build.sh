#!/bin/bash
# Build script for libcamera-bridge

set -e

echo "=== Building libcamera-bridge ==="

# Create build directory
mkdir -p build
cd build

# Configure
echo "Configuring..."
cmake ..

# Build
echo "Building..."
make -j$(nproc)

echo "=== Build complete ==="
echo "Binary: build/libcamera-bridge"
echo ""
echo "Test with:"
echo "  ./build/libcamera-bridge --width 640 --height 480 --fps 30 --bitrate 2000000 > test.h264"
