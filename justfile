
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

# Aa rtsp server receive stream
test-whipinto-rtsp:
    cargo run --bin=whipinto -- -i rtsp-listen://localhost:8550 -w http://localhost:7777/whip/test-rtsp --command \
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size=640x480:rate=30 -acodec libopus -vcodec libvpx -f rtsp 'rtsp://127.0.0.1:8550'"

# TODO:
# Aa rtsp client pull stream
test-whipinto-rtsp-pull:
    cargo run --bin=whipinto -- -i rtsp://localhost:8554/mystream -w http://localhost:7777/whip/test-rtsp

test-whepfrom-rtp:
    cargo run --bin=whepfrom -- -w http://localhost:7777/whep/test-rtp -o output.sdp --command \
        "ffplay -protocol_whitelist rtp,file,udp -i output.sdp"

# Aa rtsp server wait some pull stream
test-whepfrom-rtsp:
    cargo run --bin=whepfrom -- -w http://localhost:7777/whep/test-rtsp -o rtsp-listen://0.0.0.0:8551 --command \
        "ffplay rtsp://localhost:8551/test-rtsp"

# Aa rtsp client push stream
test-whepfrom-rtsp-push:
    cargo run --bin=whepfrom -- -w http://localhost:7777/whep/test-rtsp -o rtsp://localhost:8554/mystream



# AB stage-1: whipinto rtsp server
test-livetwo-ab1:
    cargo run --bin=whipinto -- -i rtsp-listen://localhost:8550 -w http://localhost:7777/whip/test-rtsp --command \
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size=640x480:rate=30 -acodec libopus -vcodec libvpx -f rtsp 'rtsp://127.0.0.1:8550'"

# A stage-2: whipinto rtsp server
test-livetwo-a2:
    cargo run --bin=whipinto -- -i rtsp-listen://localhost:8555 -w http://localhost:7777/whip/test-new

# A stage-3: whepfrom rtsp client
test-livetwo-a3:
    cargo run --bin=whepfrom -- -w http://localhost:7777/whep/test-rtsp -o rtsp://localhost:8555/test-new

# A stage-4: whepfrom rtsp server
test-livetwo-a4:
    cargo run --bin=whepfrom -- -w http://localhost:7777/whep/test-new -o rtsp-listen://0.0.0.0:8560 --command \
        "ffplay rtsp://localhost:8560/test-rtsp"

# B stage-2: whepfrom rtsp server
test-livetwo-b2:
    cargo run --bin=whepfrom -- -w http://localhost:7777/whep/test-rtsp -o rtsp-listen://0.0.0.0:8551

# TODO:
# B stage-3: whipinto rtsp client
test-livetwo-b3:
    cargo run --bin=whipinto -- -i rtsp://localhost:8551/mystream -w http://localhost:7777/whip/test-rtsp-2

# B stage-4: whepfrom rtsp server
test-livetwo-b4:
    cargo run --bin=whepfrom -- -w http://localhost:7777/whep/test-new -o rtsp-listen://0.0.0.0:8560 --command \
        "ffplay rtsp://localhost:8560/test-rtsp"

