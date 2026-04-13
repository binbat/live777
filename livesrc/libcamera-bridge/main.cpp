#include <iostream>
#include <cstring>
#include <csignal>
#include <atomic>
#include <getopt.h>
#include <fcntl.h>
#include <unistd.h>
#include "camera.h"
#include "encoder.h"

std::atomic<bool> running(true);

void signalHandler(int signal) {
    if (signal == SIGINT || signal == SIGTERM) {
        running = false;
    }
}

void printUsage(const char* prog) {
    std::cerr << "Usage: " << prog << " [options]\n"
              << "Options:\n"
              << "  --width WIDTH         Video width (default: 640)\n"
              << "  --height HEIGHT       Video height (default: 480)\n"
              << "  --fps FPS             Frame rate (default: 30)\n"
              << "  --bitrate BITRATE     Bitrate in bps (default: 2000000)\n"
              << "  --camera-id ID        PiCamera ID (default: 0)\n"
              << "  --rotation DEGREES    Rotation degrees: 0, 90, 180, 270 (default: 0)\n"
              << "  --hflip               Horizontal flip\n"
              << "  --vflip               Vertical flip\n"
              << "  --help                Show this help\n"
              << "\n"
              << "Control commands (stdin):\n"
              << "  k - Request keyframe (IDR)\n";
}

int main(int argc, char* argv[]) {
    // Default parameters
    CameraParams camParams = {
        .width = 640,
        .height = 480,
        .fps = 30,
        .bitrate = 2000000,
        .camera_id = 0,
        .rotation = 0,
        .hflip = false,
        .vflip = false
    };

    // Parse command line arguments
    static struct option long_options[] = {
        {"width",     required_argument, 0, 'w'},
        {"height",    required_argument, 0, 'h'},
        {"fps",       required_argument, 0, 'f'},
        {"bitrate",   required_argument, 0, 'b'},
        {"camera-id", required_argument, 0, 'c'},
        {"rotation",  required_argument, 0, 'r'},
        {"hflip",     no_argument,       0, 'H'},
        {"vflip",     no_argument,       0, 'V'},
        {"help",      no_argument,       0, '?'},
        {0, 0, 0, 0}
    };

    int opt;
    while ((opt = getopt_long(argc, argv, "w:h:f:b:c:r:HV?", long_options, nullptr)) != -1) {
        switch (opt) {
            case 'w': camParams.width = atoi(optarg); break;
            case 'h': camParams.height = atoi(optarg); break;
            case 'f': camParams.fps = atoi(optarg); break;
            case 'b': camParams.bitrate = atoi(optarg); break;
            case 'c': camParams.camera_id = atoi(optarg); break;
            case 'r': camParams.rotation = atoi(optarg); break;
            case 'H': camParams.hflip = true; break;
            case 'V': camParams.vflip = true; break;
            case '?':
            default:
                printUsage(argv[0]);
                return 1;
        }
    }

    // Setup signal handlers
    signal(SIGINT, signalHandler);
    signal(SIGTERM, signalHandler);
    
    // CRITICAL: Set stdout to unbuffered mode for pipe communication
    setbuf(stdout, NULL);
    
    // Set stdin to non-blocking mode for control commands
    int flags = fcntl(STDIN_FILENO, F_GETFL, 0);
    fcntl(STDIN_FILENO, F_SETFL, flags | O_NONBLOCK);

    std::cerr << "libcamera-bridge starting...\n"
              << "  Resolution: " << camParams.width << "x" << camParams.height << "\n"
              << "  FPS: " << camParams.fps << "\n"
              << "  Bitrate: " << camParams.bitrate << " bps\n"
              << "  Control: stdin (send 'k' for keyframe)\n";

    // Create camera
    PiCamera camera;
    if (!camera.init(camParams)) {
        std::cerr << "Failed to initialize camera: " << camera.getError() << "\n";
        return 1;
    }

    // Create encoder
    EncoderParams encParams = {
        .width = camParams.width,
        .height = camParams.height,
        .fps = camParams.fps,
        .bitrate = camParams.bitrate
    };
    
    Encoder encoder;
    if (!encoder.init(encParams)) {
        std::cerr << "Failed to initialize encoder: " << encoder.getError() << "\n";
        return 1;
    }

    // Set callbacks
    camera.setFrameCallback([&encoder](const uint8_t* data, size_t size, uint64_t timestamp) {
        // Forward raw frame to encoder
        encoder.encode(data, size, timestamp);
    });

    encoder.setNALCallback([](const uint8_t* data, size_t size, bool is_keyframe) {
        // Write H.264 NAL unit to stdout (Annex B format with start code)
        (void)is_keyframe;  // Unused for now - could be used for logging keyframes
        static const uint8_t start_code[] = {0, 0, 0, 1};
        fwrite(start_code, 1, sizeof(start_code), stdout);
        fwrite(data, 1, size, stdout);
        fflush(stdout);
    });

    // Start camera
    if (!camera.start()) {
        std::cerr << "Failed to start camera: " << camera.getError() << "\n";
        return 1;
    }

    std::cerr << "Streaming started. Press Ctrl+C to stop.\n";

    // Main loop with stdin monitoring
    while (running) {
        // Check for stdin commands (non-blocking)
        char cmd;
        ssize_t n = read(STDIN_FILENO, &cmd, 1);
        if (n > 0) {
            if (cmd == 'k' || cmd == 'K') {
                std::cerr << "⌨  Keyframe request received\n";
                encoder.forceKeyframe();
            }
            // Ignore other characters and newlines
        }
        
        usleep(10000); // 10ms - responsive to commands
    }

    // Cleanup
    std::cerr << "Stopping...\n";
    camera.stop();

    return 0;
}
