#pragma once

#include "media_types.h"

#include <cstddef>
#include <cstdint>
#include <linux/videodev2.h>

struct V4l2M2mInputLayout {
    uint32_t fourcc = 0;
    uint32_t minimum_stride = 0;
    size_t frame_bytes = 0;
};

inline bool v4l2_m2m_input_fourcc(RawPixelFormat format, uint32_t* fourcc) {
    if (!fourcc) return false;
    switch (format) {
    case RawPixelFormat::Nv12:
        *fourcc = V4L2_PIX_FMT_NV12;
        return true;
    case RawPixelFormat::Yuv420p:
        *fourcc = V4L2_PIX_FMT_YUV420;
        return true;
    case RawPixelFormat::Uyvy422:
        *fourcc = V4L2_PIX_FMT_UYVY;
        return true;
    default:
        return false;
    }
}

inline bool resolve_v4l2_m2m_input_layout(
    RawPixelFormat format, uint32_t width, uint32_t height,
    uint32_t stride, V4l2M2mInputLayout* layout) {
    if (!layout || width == 0 || height == 0) return false;

    uint32_t fourcc = 0;
    if (!v4l2_m2m_input_fourcc(format, &fourcc)) return false;

    uint32_t minimum_stride = width;
    size_t frame_bytes = 0;
    if (format == RawPixelFormat::Uyvy422) {
        if ((width & 1U) != 0) return false;
        minimum_stride = width * 2;
        if (stride < minimum_stride) return false;
        frame_bytes = static_cast<size_t>(stride) * height;
    } else {
        if ((width & 1U) != 0 || (height & 1U) != 0 || stride < width
            || (stride & 1U) != 0) {
            return false;
        }
        frame_bytes = static_cast<size_t>(stride) * height * 3 / 2;
    }

    layout->fourcc = fourcc;
    layout->minimum_stride = minimum_stride;
    layout->frame_bytes = frame_bytes;
    return true;
}
