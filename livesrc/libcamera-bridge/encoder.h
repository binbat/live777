#include <memory>

class EncoderBackend;

/// PIMPL shell — EncoderBackend implementations live in encoder.cpp /
/// encoder_rdk.cpp.  The public static factories below are the only
/// entry points for creating encoder backends.
class Encoder {
public:
    Encoder();
    ~Encoder();

    static std::unique_ptr<EncoderBackend> createV4L2M2MBackend();
    static std::unique_ptr<EncoderBackend> createRdkX5Backend();

private:
    class Impl;
    std::unique_ptr<Impl> pImpl;
};
