#include "include/v4l2_m2m_dmabuf.h"

#include <cstdio>

#define CHECK(condition)                                                        \
    do {                                                                        \
        if (!(condition)) {                                                     \
            std::fprintf(stderr, "check failed at line %d: %s\n", __LINE__,   \
                         #condition);                                           \
            return 1;                                                           \
        }                                                                       \
    } while (false)

static RawFrame make_nv12(uint32_t width, uint32_t height, uint32_t stride,
                          int fd, uint32_t uv_offset) {
    RawFrame frame{};
    frame.kind = BufferKind::DmaBuf;
    frame.format = RawPixelFormat::Nv12;
    frame.width = width;
    frame.height = height;
    frame.plane_count = 2;
    frame.planes[0] = {nullptr, stride, stride * height, fd, 0};
    frame.planes[1] = {nullptr, stride, stride * (height / 2), fd, uv_offset};
    return frame;
}

static RawFrame make_yuv420p(uint32_t width, uint32_t height, uint32_t stride,
                             int fd, uint32_t u_offset, uint32_t v_offset) {
    const uint32_t chroma_stride = stride / 2;
    RawFrame frame{};
    frame.kind = BufferKind::DmaBuf;
    frame.format = RawPixelFormat::Yuv420p;
    frame.width = width;
    frame.height = height;
    frame.plane_count = 3;
    frame.planes[0] = {nullptr, stride, stride * height, fd, 0};
    frame.planes[1] = {
        nullptr, chroma_stride, chroma_stride * (height / 2), fd, u_offset};
    frame.planes[2] = {
        nullptr, chroma_stride, chroma_stride * (height / 2), fd, v_offset};
    return frame;
}

int main() {
    size_t bytes = 0;

    // NV12, tightly packed 1280x720: importable, 1382400 bytes used.
    RawFrame nv12 = make_nv12(1280, 720, 1280, 7, 1280 * 720);
    CHECK(v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Nv12, 1280, 720, 1280, nv12, &bytes));
    CHECK(bytes == 1382400);

    // NV12 with ISP stride padding: importable when the encoder's S_FMT
    // stride matches the capture stride.
    RawFrame padded = make_nv12(1280, 720, 1312, 7, 1312 * 720);
    CHECK(v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Nv12, 1280, 720, 1312, padded, &bytes));
    CHECK(bytes == static_cast<size_t>(1312) * 720 * 3 / 2);

    // Same frame against a different encoder stride: copy fallback.
    CHECK(!v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Nv12, 1280, 720, 1280, padded, &bytes));
    CHECK(bytes == 0);

    // NV12 with the UV plane at an unexpected offset: copy fallback.
    RawFrame bad_uv = make_nv12(1280, 720, 1280, 7, 1280 * 720 + 64);
    CHECK(!v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Nv12, 1280, 720, 1280, bad_uv, &bytes));

    // NV12M (one fd per plane): cannot express as a single-plane import.
    RawFrame nv12m = make_nv12(1280, 720, 1280, 7, 0);
    nv12m.planes[1].dma_fd = 8;
    CHECK(!v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Nv12, 1280, 720, 1280, nv12m, &bytes));

    // CPU frames are never importable.
    nv12.kind = BufferKind::Cpu;
    CHECK(!v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Nv12, 1280, 720, 1280, nv12, &bytes));

    // Dimension mismatch with the encoder configuration.
    RawFrame small = make_nv12(640, 480, 640, 7, 640 * 480);
    CHECK(!v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Nv12, 1280, 720, 1280, small, &bytes));

    // Encoder configured for a different input format than the frame.
    CHECK(!v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Yuv420p, 1280, 720, 1280,
        make_nv12(1280, 720, 1280, 7, 1280 * 720), &bytes));

    // YUV420P, tightly packed 3-plane (libcamera layout): importable.
    const uint32_t y_bytes = 1280 * 720;
    const uint32_t chroma_bytes = 640 * 360;
    RawFrame i420 = make_yuv420p(1280, 720, 1280, 7, y_bytes,
                                 y_bytes + chroma_bytes);
    CHECK(v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Yuv420p, 1280, 720, 1280, i420, &bytes));
    CHECK(bytes == 1382400);

    // YUV420P with padded stride and matching offsets: importable.
    const uint32_t p_y = 1312 * 720;
    const uint32_t p_c = 656 * 360;
    RawFrame i420p = make_yuv420p(1280, 720, 1312, 7, p_y, p_y + p_c);
    CHECK(v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Yuv420p, 1280, 720, 1312, i420p, &bytes));
    CHECK(bytes == static_cast<size_t>(1312) * 720 * 3 / 2);

    // Inter-plane padding (offset not implied by the layout): copy fallback.
    RawFrame gap = make_yuv420p(1280, 720, 1280, 7, y_bytes + 64,
                                y_bytes + 64 + chroma_bytes);
    CHECK(!v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Yuv420p, 1280, 720, 1280, gap, &bytes));

    // Missing fd: copy fallback.
    RawFrame nofd = make_yuv420p(1280, 720, 1280, -1, y_bytes,
                                 y_bytes + chroma_bytes);
    CHECK(!v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Yuv420p, 1280, 720, 1280, nofd, &bytes));

    // UYVY (packed) has no multi-plane import path: copy fallback.
    RawFrame uyvy{};
    uyvy.kind = BufferKind::DmaBuf;
    uyvy.format = RawPixelFormat::Uyvy422;
    uyvy.width = 1280;
    uyvy.height = 720;
    uyvy.plane_count = 1;
    uyvy.planes[0] = {nullptr, 2560, 2560 * 720, 7, 0};
    CHECK(!v4l2_m2m_dmabuf_importable(
        RawPixelFormat::Uyvy422, 1280, 720, 2560, uyvy, &bytes));
}
