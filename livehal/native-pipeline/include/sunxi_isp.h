//! Allwinner AWIspApi lifecycle controller for the sunxi VIN capture path.
//!
//! The Allwinner VIN pipeline needs its userspace ISP (ISP602) running before
//! VIDIOC_STREAMON: CreateAWIspApi -> ispApiInit -> ispGetIspId(video_id) ->
//! ispStart(isp_id), and the reverse after STREAMOFF. All private subdev
//! ioctls, media-topology enumeration and the 3A event loop happen inside the
//! closed-source libAWIspApi.so server thread; we only drive its lifecycle
//! and must NOT configure media-ctl links (the device tree does that).
//!
//! This header is included by v4l2_capture.cpp only under ENABLE_BACKEND_SUNXI,
//! so non-sunxi builds never see it and never need the Allwinner SDK headers.

#pragma once

#include <memory>
#include <string>

class SunxiIspController {
public:
    SunxiIspController();
    ~SunxiIspController();
    SunxiIspController(const SunxiIspController&) = delete;
    SunxiIspController& operator=(const SunxiIspController&) = delete;

    /// Resolve the isp id for a /dev/videoX device and initialise the ISP
    /// library (CreateAWIspApi/ispApiInit/ispGetIspId). Call once, after the
    /// V4L2 device is set up. Returns false on a hard error.
    bool init(const std::string& device, std::string* err);

    /// ispStart(isp_id). MUST be called immediately BEFORE VIDIOC_STREAMON.
    bool start(std::string* err);

    /// ispStop/ispWaitToExit/ispApiUnInit/DestroyAWIspApi. Idempotent; MUST
    /// run AFTER VIDIOC_STREAMOFF. Safe when init() failed or start() skipped.
    void stop();

private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
