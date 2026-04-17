#include "bridge_ffi.h"
#include "camera.h"
#include <stdint.h>
#include <memory>
#include <vector>
#include <string>

class Encoder {
public:
    Encoder();
    ~Encoder();

    bool init(const CameraParams& params);
    void encode(const uint8_t* data, size_t size, uint64_t timestamp);
    void stop();
    void requestKeyframe();
    
    // Direct FFI callback pointer from Rust
    void setNALCallback(NALCallbackFFI callback, void* user_data);
    
    // Get last error message
    const char* getError() const;

private:
    class Impl;
    std::unique_ptr<Impl> pImpl;
};
