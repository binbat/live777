#!/usr/bin/env -S just --justfile

host := "127.0.0.1"
server := "http://" + host + ":7777"

isdp := "i.sdp"
osdp := "o.sdp"

asrc := "-f lavfi -i sine=frequency=1000"
vsrc := "-f lavfi -i testsrc=size=640x480:rate=30"

h264 := "libx264 -profile:v baseline -level 3.0 -pix_fmt yuv420p -g 30 -keyint_min 30 -b:v 1000k -minrate 1000k -maxrate 1000k -bufsize 1000k -preset ultrafast -tune zerolatency"
vp9  := "libvpx-vp9 -pix_fmt yuv420p"

default:
    just --list

build:
    pnpm install
    pnpm run build
    cargo build --release --all-targets --all-features

docs:
    pnpm run docs:dev

run:
    cargo run --features=webui

run-cluster:
    cargo run --bin=livenil --features=webui -- -c conf/livenil

[group('test-cycle-rtsp')]
test-cycle-rtsp-0a:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/cycle-rtsp-a --command \
        "ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -vcodec libvpx -f rtsp rtsp://{{host}}:8550"

[group('test-cycle-rtsp')]
test-cycle-rtsp-1a:
    cargo run --bin=whepfrom -- -o rtsp-listen://{{host}}:8650 -w {{server}}/whep/cycle-rtsp-a

[group('test-cycle-rtsp')]
test-cycle-rtsp-2b:
    cargo run --bin=whipinto -- -i rtsp://{{host}}:8650 -w {{server}}/whip/cycle-rtsp-b

[group('test-cycle-rtsp')]
test-cycle-rtsp-3c:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8750 -w {{server}}/whip/cycle-rtsp-c

[group('test-cycle-rtsp')]
test-cycle-rtsp-4b:
    cargo run --bin=whepfrom -- -o rtsp://{{host}}:8750 -w {{server}}/whep/cycle-rtsp-b

[group('test-cycle-rtsp')]
test-cycle-rtsp-5c:
    cargo run --bin=whepfrom -- -o rtsp-listen://{{host}}:8850 -w {{server}}/whep/cycle-rtsp-c --command \
        "ffplay rtsp://{{host}}:8850"

test-whipinto-rtp:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/test-rtp --command \
        "ffmpeg -re {{vsrc}} -vcodec libvpx -f rtp rtp://{{host}}:5002 -sdp_file {{isdp}}"

# 4K (3840×2160)
test-whipinto-rtp-4k:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/test-rtp --command \
        "ffmpeg -re -f lavfi -i testsrc=size=3840x2160:rate=30 -strict experimental -vcodec {{vp9}} -f rtp rtp://{{host}}:5002 -sdp_file {{isdp}}"

test-whipinto-rtp-h264:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/test-rtp --command \
        "ffmpeg -re {{vsrc}} -vcodec {{h264}} -f rtp rtp://{{host}}:5002 -sdp_file {{isdp}}"

test-whepfrom-rtp:
    cargo run --bin=whepfrom -- -o "rtp://localhost?video=9000&audio=9002" --sdp-file {{osdp}} -w {{server}}/whep/test-rtp --command \
        "ffplay -protocol_whitelist rtp,file,udp -i {{osdp}}"

# Aa rtsp server receive stream
test-whipinto-rtsp:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/test-rtsp --command \
        "ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -vcodec libvpx -f rtsp rtsp://{{host}}:8550"

test-whepfrom-rtsp:
    cargo run --bin=whepfrom -- -o rtsp-listen://{{host}}:8650 -w {{server}}/whep/test-rtsp --command \
        "ffplay rtsp://{{host}}:8650"

test-whipinto-rtsp-tcp:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/test-rtsp --command \
        "ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -vcodec libvpx -rtsp_transport tcp -f rtsp rtsp://{{host}}:8550"

test-whepfrom-rtsp-tcp:
    cargo run --bin=whepfrom -- -o rtsp-listen://{{host}}:8650 -w {{server}}/whep/test-rtsp --command \
        "ffplay rtsp://{{host}}:8650 -rtsp_transport tcp"

test-whipinto-rtsp-vp9:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/test-rtsp --command \
        "ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -strict experimental -vcodec {{vp9}} -f rtsp rtsp://{{host}}:8550"

test-whipinto-rtsp-h264:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/test-rtsp --command \
        "ffmpeg -re {{vsrc}} -vcodec {{h264}} -f rtsp rtsp://{{host}}:8550"

test-whipinto-rtsp-h264-raw:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/test-rtsp --command \
        "ffmpeg -re {{vsrc}} -vcodec libx264 -f rtsp rtsp://{{host}}:8550"

