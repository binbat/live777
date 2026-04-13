#!/bin/bash
# Deploy script - transfer to Raspberry Pi and build

PI_USER="hao"
PI_HOST="192.168.132.253"
PI_PATH="~/livesrc/libcamera-bridge"

echo "=== Deploying libcamera-bridge to Raspberry Pi ==="

# Create remote directory
echo "Creating remote directory..."
ssh ${PI_USER}@${PI_HOST} "mkdir -p ${PI_PATH}"

# Transfer source files
echo "Transferring files..."
scp -r \
    CMakeLists.txt \
    *.h \
    *.cpp \
    build.sh \
    README.md \
    ${PI_USER}@${PI_HOST}:${PI_PATH}/

echo "=== Transfer complete ==="
echo ""
echo "Run on Raspberry Pi:"
echo "  ssh ${PI_USER}@${PI_HOST}"
echo "  cd ${PI_PATH}"
echo "  ./build.sh"
echo "  ./build/libcamera-bridge --width 640 --height 480 --fps 30 > test.h264"
