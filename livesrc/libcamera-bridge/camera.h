#ifndef CAMERA_H
#define CAMERA_H

#include <stddef.h>
#include <stdint.h>

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

// Raw Global Callback Type
typedef void (*GlobalFrameCallback)(const uint8_t* data, size_t size, uint64_t timestamp, void* user_data);

// Opaque handle for C++ implementation
typedef void* CameraHandle;

extern "C" {
    CameraHandle camera_create();
    void camera_destroy(CameraHandle handle);
    bool camera_init(CameraHandle handle, const CameraParams* params);
    bool camera_start(CameraHandle handle);
    void camera_stop(CameraHandle handle);
    void camera_set_callback(CameraHandle handle, GlobalFrameCallback callback, void* user_data);
    const char* camera_get_error(CameraHandle handle);
}

#endif // CAMERA_H
