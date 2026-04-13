#include "camera.h"
#include <libcamera/libcamera.h>
#include <libcamera/control_ids.h>
#include <sys/mman.h>
#include <sys/ioctl.h>
#include <fcntl.h>
#include <unistd.h>
#include <linux/dma-heap.h>
#include <iostream>
#include <cstring>
#include <algorithm>

using namespace libcamera;

// DMA heap allocator
static int create_dma_allocator() {
    static const char *heap_paths[] = {
        "/dev/dma_heap/vidbuf_cached",
        "/dev/dma_heap/linux,cma",
    };
    
    for (const char* path : heap_paths) {
        int fd = open(path, O_RDWR | O_CLOEXEC, 0);
        if (fd >= 0) {
            return fd;
        }
    }
    return -1;
}

// Filter out USB cameras, keep only CSI cameras
static std::vector<std::shared_ptr<libcamera::Camera>> get_csi_cameras(CameraManager* manager) {
    std::vector<std::shared_ptr<libcamera::Camera>> cameras = manager->cameras();
    auto it = std::remove_if(cameras.begin(), cameras.end(), [](auto& cam) {
        return cam->id().find("/usb") != std::string::npos;
    });
    cameras.erase(it, cameras.end());
    
    // Sort by ID
    std::sort(cameras.begin(), cameras.end(), [](auto& a, auto& b) {
        return a->id() > b->id();
    });
    
    return cameras;
}

class PiCamera::Impl {
public:
    std::unique_ptr<CameraManager> cameraManager;
    std::shared_ptr<libcamera::Camera> camera;
    Stream* videoStream = nullptr;
    std::vector<std::unique_ptr<Request>> requests;
    std::vector<std::unique_ptr<FrameBuffer>> frameBuffers;
    std::map<FrameBuffer*, uint8_t*> mappedBuffers;
    FrameCallback frameCallback;
    std::string errorMsg;
    int fps = 30; // Store for start()
    bool running = false;

    bool initCameraManager();
    bool configureCamera(const CameraParams& params);
    bool allocateDMABuffers(const CameraParams& params, 
                           const StreamConfiguration& streamConf);
    static void onRequestCompleted(Request* request);
};

PiCamera::PiCamera() : pImpl(std::make_unique<Impl>()) {}

PiCamera::~PiCamera() {
    stop();
}

bool PiCamera::Impl::initCameraManager() {
    cameraManager = std::make_unique<CameraManager>();
    int ret = cameraManager->start();
    if (ret != 0) {
        errorMsg = "Failed to start CameraManager";
        return false;
    }
    
    return true;
}

bool PiCamera::Impl::configureCamera(const CameraParams& params) {
    // Get CSI cameras
    auto cameras = get_csi_cameras(cameraManager.get());
    if (cameras.empty()) {
        errorMsg = "No CSI cameras found";
        return false;
    }
    
    if (params.camera_id >= static_cast<int>(cameras.size())) {
        errorMsg = "Camera ID out of range";
        return false;
    }
    
    // Get camera
    camera = cameraManager->get(cameras[params.camera_id]->id());
    if (!camera) {
        errorMsg = "Failed to get camera";
        return false;
    }
    
    // Acquire camera
    int ret = camera->acquire();
    if (ret != 0) {
        errorMsg = "Failed to acquire camera";
        return false;
    }
    
    // Generate configuration for video recording
    std::unique_ptr<CameraConfiguration> config = 
        camera->generateConfiguration({StreamRole::VideoRecording});
    
    if (!config) {
        errorMsg = "Failed to generate configuration";
        return false;
    }
    
    // Configure video stream
    StreamConfiguration& streamConfig = config->at(0);
    streamConfig.size = Size(params.width, params.height);
    streamConfig.pixelFormat = formats::YUV420;
    streamConfig.bufferCount = 4;  // Use 4 buffers
    
    // Set color space based on resolution
    if (params.width >= 1280 || params.height >= 720) {
        streamConfig.colorSpace = ColorSpace::Rec709;
    } else {
        streamConfig.colorSpace = ColorSpace::Smpte170m;
    }
    
    // Apply orientation (rotation, flip)
    config->orientation = Orientation::Rotate0;
    if (params.rotation == 180) {
        config->orientation = config->orientation * Transform::Rot180;
    }
    if (params.hflip) {
        config->orientation = config->orientation * Transform::HFlip;
    }
    if (params.vflip) {
        config->orientation = config->orientation * Transform::VFlip;
    }
    
    // Validate configuration
    CameraConfiguration::Status status = config->validate();
    if (status == CameraConfiguration::Invalid) {
        errorMsg = "Configuration validation failed";
        return false;
    }
    
    // Apply configuration
    ret = camera->configure(config.get());
    if (ret != 0) {
        errorMsg = "Failed to configure camera";
        return false;
    }
    
    videoStream = streamConfig.stream();
    fps = params.fps; // Store for start()
    
    // Create requests with cookie
    for (unsigned int i = 0; i < streamConfig.bufferCount; i++) {
        std::unique_ptr<Request> request = camera->createRequest(reinterpret_cast<uint64_t>(this));
        if (!request) {
            errorMsg = "Failed to create request";
            return false;
        }
        requests.push_back(std::move(request));
    }
    
    // Allocate DMA buffers
    if (!allocateDMABuffers(params, streamConfig)) {
        return false;
    }
    
    return true;
}

bool PiCamera::Impl::allocateDMABuffers(const CameraParams& params,
                                      const StreamConfiguration& streamConf) {
    (void)params;  // Unused for now
    
    int allocatorFd = create_dma_allocator();
    if (allocatorFd < 0) {
        errorMsg = "Failed to open DMA heap allocator";
        return false;
    }
    
    // Allocate buffers for video stream
    for (size_t i = 0; i < requests.size(); i++) {
        struct dma_heap_allocation_data alloc = {};
        alloc.len = streamConf.frameSize;
        alloc.fd_flags = O_CLOEXEC | O_RDWR;
        
        int ret = ioctl(allocatorFd, DMA_HEAP_IOCTL_ALLOC, &alloc);
        if (ret < 0) {
            close(allocatorFd);
            errorMsg = "Failed to allocate DMA buffer";
            return false;
        }
        
        UniqueFD fd(alloc.fd);
        
        // Create FrameBuffer
        std::vector<FrameBuffer::Plane> planes(1);
        planes[0].fd = SharedFD(std::move(fd));
        planes[0].offset = 0;
        planes[0].length = streamConf.frameSize;
        
        frameBuffers.push_back(std::make_unique<FrameBuffer>(planes));
        FrameBuffer* fb = frameBuffers.back().get();
        
        // Map buffer for CPU access
        uint8_t* mapped = static_cast<uint8_t*>(mmap(
            nullptr, streamConf.frameSize, PROT_READ | PROT_WRITE,
            MAP_SHARED, planes[0].fd.get(), 0
        ));
        
        if (mapped == MAP_FAILED) {
            close(allocatorFd);
            errorMsg = "Failed to mmap buffer";
            return false;
        }
        
        mappedBuffers[fb] = mapped;
        
        // Add buffer to request
        ret = requests[i]->addBuffer(videoStream, fb);
        if (ret != 0) {
            close(allocatorFd);
            errorMsg = "Failed to add buffer to request";
            return false;
        }
    }
    
    close(allocatorFd);
    return true;
}

void PiCamera::Impl::onRequestCompleted(Request* request) {
    Impl* impl = reinterpret_cast<Impl*>(request->cookie());
    
    if (!impl->running) {
        return;
    }
    
    if (request->status() == Request::RequestCancelled) {
        std::cerr << "Request cancelled\n";
        return;
    }
    
    // Get frame buffer
    FrameBuffer* buffer = request->buffers().at(impl->videoStream);
    uint8_t* data = impl->mappedBuffers.at(buffer);
    size_t size = buffer->planes()[0].length;
    uint64_t timestamp = buffer->metadata().timestamp / 1000;  // Convert to microseconds
    
    std::cerr << "Camera frame received: " << size << " bytes, timestamp: " << timestamp << "\n";
    
    // Call frame callback
    if (impl->frameCallback) {
        impl->frameCallback(data, size, timestamp);
    } else {
        std::cerr << "WARNING: No frame callback set!\n";
    }
    
    // Reuse request
    request->reuse(Request::ReuseFlag::ReuseBuffers);
    impl->camera->queueRequest(request);
}

bool PiCamera::init(const CameraParams& params) {
    if (!pImpl->initCameraManager()) {
        return false;
    }
    
    return pImpl->configureCamera(params);
}

bool PiCamera::start() {
    if (pImpl->running) {
        pImpl->errorMsg = "Camera already running";
        return false;
    }
    
    // Set request completed callback
    pImpl->camera->requestCompleted.connect(pImpl.get(), &PiCamera::Impl::onRequestCompleted);
    
    // Set frame rate (FrameDurationLimits is in microseconds)
    ControlList controls;
    int64_t frame_time = 1000000 / pImpl->fps;
    controls.set(controls::FrameDurationLimits, {frame_time, frame_time});
    
    // Start camera
    int ret = pImpl->camera->start(&controls);
    if (ret != 0) {
        pImpl->errorMsg = "Failed to start camera";
        return false;
    }
    
    pImpl->running = true;
    
    // Queue all requests (cookie already set during creation)
    for (auto& request : pImpl->requests) {
        ret = pImpl->camera->queueRequest(request.get());
        if (ret != 0) {
            pImpl->errorMsg = "Failed to queue request";
            stop();
            return false;
        }
    }
    
    return true;
}

void PiCamera::stop() {
    if (!pImpl->running) {
        return;
    }
    
    pImpl->running = false;
    
    if (pImpl->camera) {
        pImpl->camera->stop();
        pImpl->camera->release();
    }
    
    // Unmap buffers
    for (auto& pair : pImpl->mappedBuffers) {
        FrameBuffer* fb = pair.first;
        uint8_t* mapped = pair.second;
        munmap(mapped, fb->planes()[0].length);
    }
    pImpl->mappedBuffers.clear();
}

void PiCamera::setFrameCallback(FrameCallback callback) {
    pImpl->frameCallback = std::move(callback);
}

const char* PiCamera::getError() const {
    return pImpl->errorMsg.c_str();
}
