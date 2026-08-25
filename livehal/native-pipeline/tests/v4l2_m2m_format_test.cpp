#include "include/v4l2_m2m_format.h"

#include <cstdio>
#include <linux/videodev2.h>

#define CHECK(condition)                                                        \
    do {                                                                        \
        if (!(condition)) {                                                     \
            std::fprintf(stderr, "check failed at line %d: %s\n", __LINE__,   \
                         #condition);                                           \
            return 1;                                                           \
        }                                                                       \
    } while (false)

int main() {
    V4l2M2mInputLayout layout{};

    CHECK(resolve_v4l2_m2m_input_layout(
        RawPixelFormat::Uyvy422, 1280, 720, 2560, &layout));
    CHECK(layout.fourcc == V4L2_PIX_FMT_UYVY);
    CHECK(layout.minimum_stride == 2560);
    CHECK(layout.frame_bytes == 1843200);

    CHECK(resolve_v4l2_m2m_input_layout(
        RawPixelFormat::Uyvy422, 1280, 720, 2688, &layout));
    CHECK(layout.fourcc == V4L2_PIX_FMT_UYVY);
    CHECK(layout.minimum_stride == 2560);
    CHECK(layout.frame_bytes == 1935360);

    CHECK(!resolve_v4l2_m2m_input_layout(
        RawPixelFormat::Uyvy422, 1279, 720, 2558, &layout));
    CHECK(!resolve_v4l2_m2m_input_layout(
        RawPixelFormat::Uyvy422, 1280, 720, 1280, &layout));
    CHECK(!resolve_v4l2_m2m_input_layout(
        RawPixelFormat::Uyvy422, 0, 720, 2560, &layout));
    CHECK(!resolve_v4l2_m2m_input_layout(
        RawPixelFormat::Uyvy422, 1280, 0, 2560, &layout));
    CHECK(!resolve_v4l2_m2m_input_layout(
        RawPixelFormat::Rgb888, 1280, 720, 3840, &layout));

    CHECK(resolve_v4l2_m2m_input_layout(
        RawPixelFormat::Nv12, 1280, 720, 1280, &layout));
    CHECK(layout.fourcc == V4L2_PIX_FMT_NV12);
    CHECK(layout.minimum_stride == 1280);
    CHECK(layout.frame_bytes == 1382400);

    CHECK(resolve_v4l2_m2m_input_layout(
        RawPixelFormat::Yuv420p, 1280, 720, 1280, &layout));
    CHECK(layout.fourcc == V4L2_PIX_FMT_YUV420);
    CHECK(layout.minimum_stride == 1280);
    CHECK(layout.frame_bytes == 1382400);
}
