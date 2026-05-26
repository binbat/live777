#include "bridge_ffi.h"
#include "camera.h"
#include <stdint.h>
#include <memory>
#include <vector>
#include <string>

class EncoderBackend;

class Encoder {
public:
    Encoder();
    ~Encoder();

    bool init(const CameraParams& params);
    void encode(const uint8_t* data, size_t size, uint64_t timestamp);
    void encodeShared(int dma_fd, size_t size, uint64_t timestamp);
    void stop();
    void requestKeyframe();

    // Direct FFI callback pointer from Rust
    void setNALCallback(NALCallbackFFI callback, void* user_data);

    // Get last error message
    const char* getError() const;

    /// Public factories for EncoderBackend — one per backend, implemented
    /// in the corresponding .cpp file where Encoder::Impl is defined.
    static std::unique_ptr<EncoderBackend> createV4L2M2MBackend();
    static std::unique_ptr<EncoderBackend> createRdkX5Backend();

private:
    class Impl;
    std::unique_ptr<Impl> pImpl;
};
