#ifndef ENCODER_H
#define ENCODER_H

#include <stdint.h>
#include <functional>
#include <memory>

// Encoder parameters
struct EncoderParams {
    int width;
    int height;
    int fps;
    int bitrate;
};

// H.264 NAL unit callback
using NALCallback = std::function<void(const uint8_t* data, size_t size, bool is_keyframe)>;

// H.264 Hardware Encoder (V4L2 M2M)
class Encoder {
public:
    Encoder();
    ~Encoder();

    // Initialize encoder
    bool init(const EncoderParams& params);
    
    // Encode a frame
    bool encode(const uint8_t* data, size_t size, uint64_t timestamp);
    
    // Set NAL unit callback
    void setNALCallback(NALCallback callback);
    
    // Get last error
    const char* getError() const;
    
    // Force keyframe (IDR) generation
    bool forceKeyframe();

private:
    class Impl;
    std::unique_ptr<Impl> pImpl;
};

#endif // ENCODER_H
