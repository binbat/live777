
default:
    just --list

build:
    npm install
    npm run build
    cargo build --release --all-targets --all-features

docs:
    npm run docs:dev

run:
    cargo run --features=webui

run-cluster:
    cargo run --bin=livenil --features=webui -- -c conf/livenil

test-whipinto-rtp:
    cargo run --bin=whipinto -- -i input.sdp -w http://localhost:7777/whip/test-rtp --command \
        "ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp"

# 4K (3840×2160)
test-whipinto-rtp-4k:
    cargo run --bin=whipinto -- -i input.sdp -w http://localhost:7777/whip/test-rtp --command \
        "ffmpeg -re -f lavfi -i testsrc=size=3840x2160:rate=30 -strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p -f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp"

test-whipinto-rtp-h264:
    cargo run --bin=whipinto -- -i input.sdp -w http://localhost:7777/whip/test-rtp --command \
        "ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libx264 -profile:v baseline -level 3.0 -pix_fmt yuv420p -g 30 -keyint_min 30 -b:v 1000k -minrate 1000k -maxrate 1000k -bufsize 1000k -preset ultrafast -tune zerolatency -f rtp 'rtp://127.0.0.1:5002' -sdp_file input.sdp"

# Aa rtsp server receive stream
test-whipinto-rtsp:
    cargo run --bin=whipinto -- -i rtsp-listen://localhost:8550 -w http://localhost:7777/whip/test-rtsp --command \
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size=640x480:rate=30 -acodec libopus -vcodec libvpx -f rtsp 'rtsp://127.0.0.1:8550'"

test-whepfrom-rtsp:
    cargo run --bin=whepfrom -- -o rtsp-listen://localhost:8650 -w http://localhost:7777/whep/test-rtsp --command \
        "ffplay rtsp://localhost:8650"

test-whipinto-rtsp-tcp:
    cargo run --bin=whipinto -- -i rtsp-listen://localhost:8550 -w http://localhost:7777/whip/test-rtsp --command \
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size=640x480:rate=30 -acodec libopus -vcodec libvpx -rtsp_transport tcp -f rtsp 'rtsp://127.0.0.1:8550'"

test-whepfrom-rtsp-tcp:
    cargo run --bin=whepfrom -- -o rtsp-listen://localhost:8650 -w http://localhost:7777/whep/test-rtsp --command \
        "ffplay rtsp://localhost:8650 -rtsp_transport tcp"

test-whipinto-rtsp-vp9:
    cargo run --bin=whipinto -- -i rtsp-listen://localhost:8550 -w http://localhost:7777/whip/test-rtsp --command \
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size=640x480:rate=30 -acodec libopus -strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p -f rtsp 'rtsp://127.0.0.1:8550'"

test-whipinto-rtsp-h264:
    cargo run --bin=whipinto -- -i rtsp-listen://127.0.0.1:8550 -w http://localhost:7777/whip/test-rtsp --command \
        "ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libx264 -profile:v baseline -level 3.0 -pix_fmt yuv420p -g 30 -keyint_min 30 -b:v 1000k -minrate 1000k -maxrate 1000k -bufsize 1000k -preset ultrafast -tune zerolatency -f rtsp 'rtsp://127.0.0.1:8550'"

test-whipinto-rtsp-h264-raw:
    cargo run --bin=whipinto -- -i rtsp-listen://127.0.0.1:8550 -w http://localhost:7777/whip/test-rtsp --command \
        "ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libx264 -f rtsp 'rtsp://127.0.0.1:8550'"

