#pragma once

#include "media_types.h"

#include <cstddef>
#include <cstdint>

// ---------------------------------------------------------------------------
// DMA-BUF import check for the V4L2 M2M encoder zero-copy input path.
//
// The encoder's OUTPUT queue is a single-plane format (NV12 / YUV420) whose
// layout is fixed at S_FMT time: plane i sits at a driver-implied offset
// derived from width/height/bytesperline.  A captured dmabuf can only be
// queued as-is when the capture buffer's layout matches that exactly —
// same fd for all planes, matching strides, matching plane offsets.  Any
// deviation (ISP stride padding, inter-plane padding, multi-fd MPLANE
// capture) must take the copy fallback instead.
//
// Intentionally free of <linux/videodev2.h> so the unit test builds on any
// host platform.
// ---------------------------------------------------------------------------

inline bool v4l2_m2m_dmabuf_importable(
    RawPixelFormat input_format, uint32_t width, uint32_t height,
    uint32_t encoder_stride, const RawFrame& frame, size_t* bytes_used) {
    if (bytes_used) *bytes_used = 0;
    if (frame.kind != BufferKind::DmaBuf) return false;
    if (frame.width != width || frame.height != height) return false;
    if ((width & 1U) != 0 || (height & 1U) != 0) return false;
    if (encoder_stride < width || (encoder_stride & 1U) != 0) return false;
    if (frame.planes[0].dma_fd < 0 || frame.planes[0].offset != 0) return false;

    const int dma_fd = frame.planes[0].dma_fd;
    const size_t y_bytes = static_cast<size_t>(encoder_stride) * height;

    if (input_format == RawPixelFormat::Nv12) {
        if (frame.format != RawPixelFormat::Nv12 || frame.plane_count != 2) {
            return false;
        }
        const size_t uv_offset = y_bytes;
        if (frame.planes[0].stride != encoder_stride
            || frame.planes[1].stride != encoder_stride
            || frame.planes[1].dma_fd != dma_fd
            || frame.planes[1].offset != uv_offset) {
            return false;
        }
        if (bytes_used) *bytes_used = y_bytes + y_bytes / 2;
        return true;
    }

    if (input_format == RawPixelFormat::Yuv420p) {
        if (frame.format != RawPixelFormat::Yuv420p || frame.plane_count != 3) {
            return false;
        }
        const uint32_t chroma_stride = encoder_stride / 2;
        const size_t chroma_bytes =
            static_cast<size_t>(chroma_stride) * (height / 2);
        if (frame.planes[0].stride != encoder_stride
            || frame.planes[1].stride != chroma_stride
            || frame.planes[2].stride != chroma_stride
            || frame.planes[1].dma_fd != dma_fd
            || frame.planes[2].dma_fd != dma_fd
            || frame.planes[1].offset != y_bytes
            || frame.planes[2].offset != y_bytes + chroma_bytes) {
            return false;
        }
        if (bytes_used) *bytes_used = y_bytes + chroma_bytes * 2;
        return true;
    }

    return false;
}
