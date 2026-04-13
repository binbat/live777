#ifndef CAMERA_H
#define CAMERA_H

#include <stdint.h>
#include <functional>
#include <memory>

// Forward declarations
namespace libcamera {
    class Camera;
    class CameraManager;
    class Stream;
    class Request;
    class FrameBuffer;
}

// Camera parameters
struct CameraParams {
    int width;
    int height;
    int fps;
    int bitrate;
    int camera_id;
    int rotation;
    bool hflip;
    bool vflip;
};

// H.264 frame callback
using FrameCallback = std::function<void(const uint8_t* data, size_t size, uint64_t timestamp)>;

// PiCamera class - wrapper for libcamera
class PiCamera {
public:
    PiCamera();
    ~PiCamera();

    // Initialize camera with parameters
    bool init(const CameraParams& params);
    
    // Start capturing
    bool start();
    
    // Stop capturing
    void stop();
    
    // Set frame callback
    void setFrameCallback(FrameCallback callback);
    
    // Get last error message
    const char* getError() const;

private:
    class Impl;
    std::unique_ptr<Impl> pImpl;
};

#endif // CAMERA_H
