# libcamera-bridge

Direct libcamera API integration for LiveSrc - bypasses rpicam-vid and ffmpeg for lower latency.

## Architecture

```
Raspberry Pi Camera (CSI)
    ↓
libcamera API
    ↓
Raw Frames (DMA Buffer)
    ↓
V4L2 M2M H.264 Encoder (GPU)
    ↓
H.264 NAL Units → stdout
```

## Build Requirements

- libcamera-dev
- CMake >= 3.15
- C++17 compiler

## Build

```bash
mkdir build
cd build
cmake ..
make
```

## Usage

```bash
./libcamera-bridge \
  --width 640 \
  --height 480 \
  --fps 30 \
  --bitrate 2000000 \
  > output.h264
```

## Integration with LiveSrc

LibcameraSource will spawn this process and read H.264 from stdout.

## Status

🚧 **Work in Progress**

- [x] Project structure
- [x] CMake configuration
- [x] CLI interface
- [ ] libcamera API integration (camera.cpp)
- [ ] V4L2 M2M encoder (encoder.cpp)
- [ ] DMA buffer management
- [ ] Testing on Raspberry Pi

## Reference

Based on [mediamtx-rpicamera](https://github.com/bluenviron/mediamtx-rpicamera)
