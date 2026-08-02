#!/usr/bin/env -S just --justfile

host := "127.0.0.1"
port := "7777"
server := "http://" + host + ":" + port
stream := "test-stream"

isdp := "i.sdp"
osdp := "o.sdp"

rtsp_port := "8554"
rtsps := "rtsp://" + host + ":" + rtsp_port

irtp := "5002"
ortp := "5006"

asrc := "-f lavfi -i sine=frequency=1000"
vsrc := "-f lavfi -i testsrc=size=1280x720:rate=30"

h264 := "libx264 -pix_fmt yuv420p -g 60 -keyint_min 60 -crf 23 -preset ultrafast -tune zerolatency -profile:v main -level 4.1"
h265 := "libx265 -pix_fmt yuv420p -g 60 -keyint_min 60 -crf 25 -preset ultrafast -tune zerolatency -profile:v main -level 4.1"
vp8  := "libvpx -pix_fmt yuv420p -g 60 -keyint_min 60 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k"
vp9  := "libvpx-vp9 -pix_fmt yuv420p -g 60 -keyint_min 60 -deadline realtime -speed 5 -row-mt 1 -tile-columns 2 -frame-parallel 1 -b:v 1800k -maxrate 2200k -bufsize 4400k"
av1  := "libaom-av1 -pix_fmt yuv420p -cpu-used 8 -tile-columns 0 -tile-rows 0 -row-mt 1 -lag-in-frames 0 -g 30 -keyint_min 30 -b:v 0 -crf 30 -threads 4 -strict experimental"
opus := "libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained"

gst_hd := "video/x-raw,format=I420,width=1280,height=720,framerate=30/1"

gst_x264 := "x264enc tune=zerolatency speed-preset=ultrafast key-int-max=60 byte-stream=true"
gst_x265 := "x265enc tune=zerolatency speed-preset=ultrafast key-int-max=60 qp=23"
gst_vp8 := "vp8enc deadline=1 cpu-used=6 lag-in-frames=0 end-usage=cbr keyframe-max-dist=60"
gst_vp9 := "vp9enc deadline=1 cpu-used=6 lag-in-frames=0 end-usage=cbr keyframe-max-dist=60 row-mt=1"
gst_av1 := "av1enc usage-profile=realtime"

default:
    just --list

build:
    pnpm install
    pnpm run build
    cargo build --release --all-targets --all-features

# Size-optimized build: fat LTO, 1 codegen unit, stripped, panic=abort
# (same feature set as the release workflow)
[group('build')]
build-size:
    cargo build --profile release-size --bins \
        --features source-all,webui,net4mqtt,recorder,cascade,whepwright,target-whip

# Extreme size build: build-size + UPX LZMA (needs upx installed)
[group('build')]
pack-size: build-size
    upx --best --lzma target/release-size/live777 target/release-size/liveman target/release-size/whepfrom target/release-size/whipinto target/release-size/net4mqtt

# Examples:
#   just cross-build-size aarch64-unknown-linux-gnu native-rpi,webui   # Raspberry Pi (needs RPI_SYSROOT)
#   just cross-build-size armv7-unknown-linux-gnueabihf native-generic-v4l2,webui
#   just cross-build-size aarch64-unknown-linux-musl webui             # static musl, no native capture
# Cross-compile a size-optimized live777 for an embedded target
# (needs cross <https://github.com/cross-rs/cross> and docker)
[group('build')]
cross-build-size target features="webui":
    cross build --target {{target}} --bin live777 --profile release-size \
        --no-default-features --features {{features}}

# cross-build-size + UPX LZMA; UPX packs foreign-arch ELFs directly from the host
[group('build')]
cross-pack-size target features="webui": (cross-build-size target features)
    upx --best --lzma target/{{target}}/release-size/live777 || echo "warning: UPX cannot pack {{target}}, leaving the binary unpacked"

# Raspberry Pi (native-rpi: libcamera + V4L2 capture, V4L2 M2M encoder)
[group('embedded')]
rpi-sync-sysroot host="raspberrypi" sysroot="target/rpi-sysroot":
    #!/usr/bin/env bash
    set -euo pipefail

    remote="{{host}}"
    sysroot="{{sysroot}}"
    triplet="aarch64-linux-gnu"

    if ! ssh "$remote" "pkg-config --exists libcamera"; then
        echo "error: libcamera.pc was not found on $remote"
        echo "install libcamera-dev on the Raspberry Pi, then run this recipe again"
        exit 1
    fi

    rm -rf "$sysroot"
    mkdir -p "$sysroot/usr/lib/$triplet" "$sysroot/usr/lib/$triplet/pkgconfig"
    sysroot_abs=$(cd "$sysroot" && pwd)

    pc_dir=$(ssh "$remote" "pkg-config --variable=pcfiledir libcamera")
    mkdir -p "$sysroot$pc_dir"
    pc_modules=$(ssh "$remote" "printf '%s\n' libcamera; pkg-config --print-requires --print-requires-private libcamera" | awk '{print $1}' | sort -u)
    while read -r module; do
        [[ -n "$module" ]] || continue
        rsync -a "$remote:$pc_dir/$module.pc" "$sysroot$pc_dir/"
    done <<< "$pc_modules"

    ssh "$remote" "pkg-config --cflags-only-I libcamera" | tr ' ' '\n' | sed -n 's/^-I//p' | while read -r path; do
        [[ -n "$path" ]] || continue
        mkdir -p "$sysroot$(dirname "$path")"
        rsync -a "$remote:$path/" "$sysroot$path/"
    done

    lib_dirs=$(ssh "$remote" "pkg-config --libs-only-L libcamera" | tr ' ' '\n' | sed -n 's/^-L//p')
    if [[ -z "$lib_dirs" ]]; then
        lib_dirs="/usr/lib/$triplet"
    fi
    ssh "$remote" "pkg-config --libs-only-l libcamera" | tr ' ' '\n' | sed -n 's/^-l//p' | while read -r lib; do
        [[ -n "$lib" ]] || continue
        copied=0
        while read -r dir; do
            [[ -n "$dir" ]] || continue
            matches=$(ssh "$remote" "find '$dir' -maxdepth 1 -name 'lib$lib.so*' -print 2>/dev/null")
            [[ -n "$matches" ]] || continue
            mkdir -p "$sysroot$dir"
            while read -r file; do
                rsync -a --links "$remote:$file" "$sysroot$dir/"
                copied=1
            done <<< "$matches"
        done <<< "$lib_dirs"
        if [[ "$copied" -ne 1 ]]; then
            echo "error: lib$lib.so was not found on $remote"
            exit 1
        fi
    done

    dep_files=$(ssh "$remote" "ldd /usr/lib/$triplet/libcamera.so /usr/lib/$triplet/libcamera-base.so 2>/dev/null" \
        | awk '/=> \// { print $3 } /^\// { print $1 }' \
        | sed 's/:$//' \
        | sort -u)
    while read -r file; do
        [[ -n "$file" ]] || continue
        mkdir -p "$sysroot$(dirname "$file")"
        rsync -aL "$remote:$file" "$sysroot$(dirname "$file")/"
    done <<< "$dep_files"

    PKG_CONFIG_SYSROOT_DIR="$sysroot" \
        PKG_CONFIG_PATH="$sysroot/usr/lib/$triplet/pkgconfig" \
        PKG_CONFIG_ALLOW_CROSS=1 \
        pkg-config --exists libcamera
    find "$sysroot" -name 'libcamera.so*' -print -quit | grep -q .

    echo "RPI_SYSROOT=$sysroot_abs"

[group('embedded')]
rpi-cross-build sysroot="target/rpi-sysroot":
    #!/usr/bin/env bash
    set -euo pipefail

    if [[ ! -d "{{sysroot}}" ]]; then
        echo "error: {{sysroot}} does not exist"
        echo "run: just rpi-sync-sysroot <pi-ssh-host> {{sysroot}}"
        exit 1
    fi
    sysroot=$(cd "{{sysroot}}" && pwd)
    RPI_SYSROOT="$sysroot" cross build --target aarch64-unknown-linux-gnu \
        --bin live777 --release \
        --no-default-features --features native-rpi,webui

[group('embedded')]
rpi-sync-and-cross-build host="raspberrypi" sysroot="target/rpi-sysroot":
    just rpi-sync-sysroot {{host}} {{sysroot}}
    just rpi-cross-build {{sysroot}}

[group('embedded')]
rpi-pack-size:
    test -n "${RPI_SYSROOT:?set RPI_SYSROOT to the Raspberry Pi sysroot first (see AGENTS.md)}" && \
        just cross-pack-size aarch64-unknown-linux-gnu native-rpi,webui

# RDK X5 (native-rdk: V4L2 capture, RDK BPU encoder)
[group('embedded')]
rdk-pack-size:
    test -n "${RDK_SYSROOT:?set RDK_SYSROOT to the RDK sysroot first (see AGENTS.md)}" && \
        just cross-pack-size aarch64-unknown-linux-gnu native-rdk,webui

# Generic V4L2 device (native-generic-v4l2; override the target for 64-bit boards)
[group('embedded')]
v4l2-pack-size target="armv7-unknown-linux-gnueabihf":
    just cross-pack-size {{target}} native-generic-v4l2,webui

# MacOS:
#   brew install gstreamer
# Debian:
#   apt install libgstreamer1.0-dev libgstrtspserver-1.0-dev
#
# Build some tools: test-rtsp-server
build-tools:
    gcc -o test-rtsp-server tools/test-rtsp-server.c $(pkg-config --cflags --libs gstreamer-1.0 gstreamer-rtsp-server-1.0)

mtx_os := if os() == "macos" { "darwin" } else if os() == "linux" { "linux" } else if os() == "windows" { "windows" } else { "unsupported" }
mtx_arch := if arch() == "x86_64" { "amd64" } else if arch() == "aarch64" { "arm64" } else { "unsupported" }

# Download the mediamtx binary used by the interop matrix tests into target/
# The pinned version lives in mediamtx.version (shared with the CI action).
mediamtx:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(tr -d '[:space:]' < mediamtx.version)
    target={{mtx_os}}_{{mtx_arch}}
    if [[ "$target" == *unsupported* ]]; then
        echo "unsupported platform: {{os()}}-{{arch()}}"
        exit 1
    fi
    ext=tar.gz
    bin=mediamtx
    if [[ "{{mtx_os}}" == windows ]]; then ext=zip; bin=mediamtx.exe; fi
    fname="mediamtx_${version}_${target}.${ext}"
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    curl -fsSL -o "$tmp/$fname" "https://github.com/bluenviron/mediamtx/releases/download/${version}/${fname}"
    curl -fsSL -o "$tmp/checksums.sha256" "https://github.com/bluenviron/mediamtx/releases/download/${version}/checksums.sha256"
    grep "$fname" "$tmp/checksums.sha256" > "$tmp/check.txt"
    (cd "$tmp" && if command -v sha256sum >/dev/null; then sha256sum --check check.txt; else shasum -a 256 --check check.txt; fi)
    mkdir -p "$tmp/x" target
    if [[ "$ext" == zip ]]; then
        (cd "$tmp" && powershell -NoProfile -Command "Expand-Archive -Force '$fname' x")
    else
        tar -xzf "$tmp/$fname" -C "$tmp/x" "$bin"
    fi
    install -m 0755 "$tmp/x/$bin" "target/$bin"
    "target/$bin" --version

docs:
    pnpm run docs:dev

run:
    cargo run --features=webui

run-cluster:
    cargo run --bin=livenil --features=webui -- -c conf/livenil

only-mpeg-rtp-h264:
    ffmpeg -re {{vsrc}} -vcodec {{h264}} -f rtp 'rtp://{{host}}:5002?pkt_size=1200' -sdp_file {{isdp}}


[group('gst-whip-rtp')]
gst-whip-rtp-h264:
    #!/usr/bin/env bash
    cat > {{isdp}} << EOF
    v=0
    o=- 0 0 IN IP4 {{host}}
    s=H264 Test Stream
    c=IN IP4 {{host}}
    t=0 0
    m=video {{irtp}} RTP/AVP 96
    a=rtpmap:96 H264/90000
    EOF
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "gst-launch-1.0 videotestsrc is-live=true ! {{gst_hd}} ! {{gst_x264}} ! h264parse ! rtph264pay ! udpsink host={{host}} port={{irtp}}"
    rm {{isdp}}


[group('gst-whip-rtp')]
gst-whip-rtp-h265:
    #!/usr/bin/env bash
    cat > {{isdp}} << EOF
    v=0
    o=- 0 0 IN IP4 {{host}}
    s=H265 Test Stream
    c=IN IP4 {{host}}
    t=0 0
    m=video {{irtp}} RTP/AVP 96
    a=rtpmap:96 H265/90000
    EOF
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "gst-launch-1.0 videotestsrc is-live=true ! {{gst_hd}} ! {{gst_x265}} ! h265parse config-interval=1 ! rtph265pay ! udpsink host={{host}} port={{irtp}}"
    rm {{isdp}}

[group('gst-whip-rtp')]
gst-whip-rtp-vp8:
    #!/usr/bin/env bash
    cat > {{isdp}} << EOF
    v=0
    o=- 0 0 IN IP4 {{host}}
    s=VP8 Test Stream
    c=IN IP4 {{host}}
    t=0 0
    m=video {{irtp}} RTP/AVP 96
    a=rtpmap:96 VP8/90000
    EOF
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "gst-launch-1.0 videotestsrc is-live=true ! {{gst_hd}} ! {{gst_vp8}} ! rtpvp8pay ! udpsink host={{host}} port={{irtp}}"
    rm {{isdp}}

[group('gst-whip-rtp')]
gst-whip-rtp-vp9:
    #!/usr/bin/env bash
    cat > {{isdp}} << EOF
    v=0
    o=- 0 0 IN IP4 {{host}}
    s=VP9 Test Stream
    c=IN IP4 {{host}}
    t=0 0
    m=video {{irtp}} RTP/AVP 96
    a=rtpmap:96 VP9/90000
    EOF
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "gst-launch-1.0 videotestsrc is-live=true ! {{gst_hd}} ! {{gst_vp9}} ! vp9parse ! rtpvp9pay ! udpsink host={{host}} port={{irtp}}"
    rm {{isdp}}

# TODO: webui can't player
[group('gst-whip-rtp')]
gst-whip-rtp-av1:
    #!/usr/bin/env bash
    cat > {{isdp}} << EOF
    v=0
    o=- 0 0 IN IP4 {{host}}
    s=AV1 Test Stream
    c=IN IP4 {{host}}
    t=0 0
    m=video {{irtp}} RTP/AVP 96
    a=rtpmap:96 AV1/90000
    EOF
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "gst-launch-1.0 videotestsrc is-live=true ! {{gst_hd}} ! {{gst_av1}} ! av1parse ! rtpav1pay ! udpsink host={{host}} port={{irtp}}"
    rm {{isdp}}

# TODO: webui can't player
[group('gst-whip-rtp')]
gst-whip-rtp-opus:
    #!/usr/bin/env bash
    cat > {{isdp}} << EOF
    v=0
    o=- 0 0 IN IP4 {{host}}
    s=OPUS Test Stream
    c=IN IP4 {{host}}
    t=0 0
    m=audio {{irtp}} RTP/AVP 96
    a=rtpmap:96 OPUS/48000/2
    EOF
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "gst-launch-1.0 audiotestsrc is-live=true ! opusenc ! opusparse ! rtpopuspay ! udpsink host={{host}} port={{irtp}}"
    rm {{isdp}}

[group('gst-whip-rtp')]
gst-whip-rtp-g722:
    #!/usr/bin/env bash
    cat > {{isdp}} << EOF
    v=0
    o=- 0 0 IN IP4 {{host}}
    s=G722 Test Stream
    c=IN IP4 {{host}}
    t=0 0
    m=audio {{irtp}} RTP/AVP 96
    a=rtpmap:96 G722/8000/1
    EOF
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "gst-launch-1.0 audiotestsrc is-live=true ! avenc_g722 ! rtpg722pay ! udpsink host={{host}} port={{irtp}}"
    rm {{isdp}}

[group('gst-whip-rtp')]
gst-whip-rtp-h264-g722:
    #!/usr/bin/env bash
    cat > {{isdp}} << EOF
    v=0
    o=- 0 0 IN IP4 {{host}}
    s=H264 + G722 Test Stream
    c=IN IP4 {{host}}
    t=0 0
    m=video 5002 RTP/AVP 96
    a=rtpmap:96 H264/90000
    a=fmtp:96 packetization-mode=1
    m=audio 5004 RTP/AVP 97
    a=rtpmap:97 G722/8000
    EOF
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "gst-launch-1.0 videotestsrc is-live=true ! {{gst_hd}} ! {{gst_x264}} ! h264parse ! rtph264pay pt=96 ! udpsink host={{host}} port=5002 audiotestsrc is-live=true ! avenc_g722 ! rtpg722pay pt=97 ! udpsink host={{host}} port=5004"
    rm {{isdp}}

# TODO: only audio in webui can't player
[group('gst-whip-rtp')]
gst-whip-rtp-vp8-opus:
    #!/usr/bin/env bash
    cat > {{isdp}} << EOF
    v=0
    o=- 0 0 IN IP4 {{host}}
    s=VP8 + OPUS Test Stream
    c=IN IP4 {{host}}
    t=0 0
    m=video 5002 RTP/AVP 96
    a=rtpmap:96 VP8/90000
    m=audio 5004 RTP/AVP 97
    a=rtpmap:97 OPUS/48000/2
    EOF
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "gst-launch-1.0 -v videotestsrc is-live=true ! {{gst_hd}} ! {{gst_vp8}} ! rtpvp8pay pt=96 ! udpsink host={{host}} port=5002 audiotestsrc is-live=true ! opusenc ! opusparse ! rtpopuspay pt=97 ! udpsink host={{host}} port=5004"
    rm {{isdp}}

[group('gst-rtsp-server')]
gst-rtsp-server-h264:
    ./test-rtsp-server "( videotestsrc is-live=true ! {{gst_hd}} ! {{gst_x264}} ! h264parse ! rtph264pay name=pay0 pt=96 )"

[group('gst-rtsp-server')]
gst-rtsp-server-h265:
    ./test-rtsp-server "( videotestsrc is-live=true ! {{gst_hd}} ! {{gst_x265}} ! h265parse ! rtph265pay name=pay0 pt=96 )"

[group('gst-rtsp-server')]
gst-rtsp-server-vp8:
    ./test-rtsp-server "( videotestsrc is-live=true ! {{gst_hd}} ! {{gst_vp8}} ! rtpvp8pay name=pay0 pt=96 )"

[group('gst-rtsp-server')]
gst-rtsp-server-vp9:
    ./test-rtsp-server "( videotestsrc is-live=true ! {{gst_hd}} ! {{gst_vp9}} ! vp9parse ! rtpvp9pay name=pay0 pt=96 )"

[group('gst-rtsp-server')]
gst-rtsp-server-av1:
    ./test-rtsp-server "( videotestsrc is-live=true ! {{gst_hd}} ! {{gst_av1}} ! av1parse ! rtpav1pay name=pay0 pt=96 )"

[group('gst-rtsp-server')]
gst-rtsp-server-opus:
    ./test-rtsp-server "( audiotestsrc is-live=true ! opusenc ! opusparse ! rtpopuspay name=pay0 pt=96 )"

[group('gst-rtsp-server')]
gst-rtsp-server-g722:
    ./test-rtsp-server "( audiotestsrc is-live=true ! avenc_g722 ! rtpg722pay name=pay0 pt=96 )"

[group('gst-rtsp-server')]
gst-rtsp-server-both-h264-opus:
    ./test-rtsp-server "( videotestsrc is-live=true ! {{gst_x264}} ! rtph264pay name=pay0 pt=96 audiotestsrc is-live=true ! opusenc ! rtpopuspay name=pay1 pt=97 )"

[group('gst-rtsp-server')]
whip-rtsp:
    cargo run --bin=whipinto -- -i rtsp://{{host}}:8554/test -w {{server}}/whip/{{stream}}

[group('gst-rtsp-server')]
whip-rtp:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}}

[group('simple-rtp')]
ffmpeg-rtp-h264:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{vsrc}} -vcodec {{h264}} -f rtp 'rtp://{{host}}:5002' -sdp_file {{isdp}}"

[group('simple-rtp')]
ffmpeg-rtp-h265:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{vsrc}} -vcodec {{h265}} -f rtp 'rtp://{{host}}:5002' -sdp_file {{isdp}}"

[group('simple-rtp')]
ffmpeg-rtp-vp8:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{vsrc}} -vcodec {{vp8}} -f rtp rtp://{{host}}:5002 -sdp_file {{isdp}}"

[group('simple-rtp')]
ffmpeg-rtp-vp9:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{vsrc}} -strict experimental -vcodec {{vp9}} -f rtp rtp://{{host}}:5002 -sdp_file {{isdp}}"

[group('simple-rtp')]
ffmpeg-rtp-av1:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{vsrc}} -vcodec {{av1}} -f rtp rtp://{{host}}:5002 -sdp_file {{isdp}}"

# 4K (3840×2160)
[group('simple-rtp')]
ffmpeg-rtp-4k:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re -f lavfi -i testsrc=size=3840x2160:rate=30 -strict experimental -vcodec {{vp9}} -f rtp rtp://{{host}}:5002 -sdp_file {{isdp}}"

[group('simple-rtp')]
ffmpeg-rtp-opus:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{asrc}} -acodec {{opus}} -f rtp rtp://{{host}}:5002 -sdp_file {{isdp}}"

[group('simple-rtp')]
ffmpeg-rtp-g722:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{asrc}} -acodec g722 -f rtp rtp://{{host}}:5002?pkt_size=1200 -sdp_file {{isdp}}"

[group('simple-rtp')]
ffmpeg-rtp-vp8-opus:
    cargo run --bin=whipinto -- -i {{isdp}} -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{asrc}} {{vsrc}} -acodec {{opus}} -vn -f rtp rtp://{{host}}:5002 -vcodec libvpx -an -f rtp rtp://{{host}}:5004 -sdp_file {{isdp}}"

[group('simple-rtp')]
ffplay-rtp:
    cargo run --bin=whepfrom -- -o "rtp://localhost?video=9000&audio=9002" --sdp-file {{osdp}} -w {{server}}/whep/{{stream}} --command \
        "ffplay -protocol_whitelist rtp,file,udp -i {{osdp}}"


# Push a stream into liveion's built-in RTSP server
[group('simple-rtsp')]
ffmpeg-rtsp:
    ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -vcodec libvpx -f rtsp {{rtsps}}/{{stream}}

[group('simple-rtsp')]
ffmpeg-rtsp-tcp:
    ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -vcodec libvpx -rtsp_transport tcp -f rtsp {{rtsps}}/{{stream}}

[group('simple-rtsp')]
ffmpeg-rtsp-vp9:
    ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -strict experimental -vcodec {{vp9}} -f rtsp {{rtsps}}/{{stream}}

[group('simple-rtsp')]
ffmpeg-rtsp-h264:
    ffmpeg -re {{vsrc}} -vcodec {{h264}} -f rtsp {{rtsps}}/{{stream}}

ffmpeg-rtsp-h264-raw:
    ffmpeg -re {{vsrc}} -vcodec libx264 -f rtsp {{rtsps}}/{{stream}}

[group('simple-rtsp')]
ffmpeg-rtsp-h265:
    ffmpeg -re {{vsrc}} -vcodec {{h265}} -f rtsp {{rtsps}}/{{stream}}

[group('simple-rtsp')]
ffplay-rtsp:
    ffplay {{rtsps}}/{{stream}}

[group('simple-rtsp')]
ffplay-rtsp-tcp:
    ffplay {{rtsps}}/{{stream}} -rtsp_transport tcp


[group('cycle-rtsp')]
cycle-rtsp-0a:
    ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -vcodec libvpx -f rtsp {{rtsps}}/cycle-rtsp-a

[group('cycle-rtsp')]
cycle-rtsp-1a:
    cargo run --bin=whipinto -- -i {{rtsps}}/cycle-rtsp-a -w {{server}}/whip/cycle-rtsp-b

[group('cycle-rtsp')]
cycle-rtsp-3c:
    cargo run --bin=whipinto -- -i rtsp://{{host}}:8750 -w {{server}}/whip/cycle-rtsp-c

[group('cycle-rtsp')]
cycle-rtsp-4b:
    cargo run --bin=whepfrom -- -o rtsp://{{host}}:8750 -w {{server}}/whep/cycle-rtsp-b

[group('cycle-rtsp')]
cycle-rtsp-5c:
    ffplay {{rtsps}}/cycle-rtsp-c


# ============================================================
# ffmpeg push to liveion RTSP server (ANNOUNCE + RECORD)
# Usage: just ffmpeg-rtsp-push-h264
# ============================================================

# Push H264 test video into liveion's built-in RTSP server (ANNOUNCE + RECORD)
[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-h264:
    ffmpeg -re {{vsrc}} -vcodec {{h264}} -f rtsp {{rtsps}}/{{stream}}

[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-h265:
    ffmpeg -re {{vsrc}} -vcodec {{h265}} -f rtsp {{rtsps}}/{{stream}}

[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-vp8:
    ffmpeg -re {{vsrc}} -vcodec {{vp8}} -f rtsp {{rtsps}}/{{stream}}

[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-vp9:
    ffmpeg -re {{vsrc}} -strict experimental -vcodec {{vp9}} -f rtsp {{rtsps}}/{{stream}}

[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-av1:
    ffmpeg -re {{vsrc}} -vcodec {{av1}} -f rtsp {{rtsps}}/{{stream}}

[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-opus:
    ffmpeg -re {{asrc}} -acodec {{opus}} -f rtsp {{rtsps}}/{{stream}}

[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-g722:
    ffmpeg -re {{asrc}} -acodec g722 -f rtsp {{rtsps}}/{{stream}}

[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-h264-opus:
    ffmpeg -re {{vsrc}} {{asrc}} -vcodec {{h264}} -acodec {{opus}} -f rtsp {{rtsps}}/{{stream}}

[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-vp8-opus:
    ffmpeg -re {{vsrc}} {{asrc}} -vcodec {{vp8}} -acodec {{opus}} -f rtsp {{rtsps}}/{{stream}}

# TCP transport (force RTP over TCP interleaved)
[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-h264-tcp:
    ffmpeg -re {{vsrc}} -vcodec {{h264}} -rtsp_transport tcp -f rtsp {{rtsps}}/{{stream}}

[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-h265-tcp:
    ffmpeg -re {{vsrc}} -vcodec {{h265}} -rtsp_transport tcp -f rtsp {{rtsps}}/{{stream}}

# Push from a local file (re-wrap without re-encoding)
[group('ffmpeg-rtsp')]
ffmpeg-rtsp-push-file:
    ffmpeg -re -stream_loop -1 -i input.mp4 -c copy -f rtsp {{rtsps}}/{{stream}}


# ============================================================
# ffplay pull from liveion RTSP server (DESCRIBE + PLAY)
# Usage: just ffplay-rtsp-pull
# ============================================================

# Pull and play an RTSP stream from liveion with ffplay (DESCRIBE + PLAY)
[group('ffplay-rtsp')]
ffplay-rtsp-pull:
    ffplay {{rtsps}}/{{stream}}

[group('ffplay-rtsp')]
ffplay-rtsp-pull-tcp:
    ffplay -rtsp_transport tcp {{rtsps}}/{{stream}}

[group('ffplay-rtsp')]
ffplay-rtsp-pull-lowlatency:
    ffplay -rtsp_transport tcp -fflags nobuffer -flags low_delay -framedrop {{rtsps}}/{{stream}}

[group('ffplay-rtsp')]
ffplay-rtsp-pull-novideo:
    ffplay -vn {{rtsps}}/{{stream}}

[group('ffplay-rtsp')]
ffplay-rtsp-pull-noaudio:
    ffplay -an {{rtsps}}/{{stream}}


# ============================================================
# ffprobe inspect RTSP stream from liveion
# Usage: just ffprobe-rtsp
# ============================================================

# Inspect an RTSP stream from liveion with ffprobe (JSON stream info)
[group('ffprobe-rtsp')]
ffprobe-rtsp:
    ffprobe -v error -hide_banner -i {{rtsps}}/{{stream}} -show_streams -of json

[group('ffprobe-rtsp')]
ffprobe-rtsp-tcp:
    ffprobe -rtsp_transport tcp -v error -hide_banner -i {{rtsps}}/{{stream}} -show_streams -of json

# ============================================================
# loadtest: WHIP publish / WHEP subscribe / DataChannel benchmarks
# Usage: just livewrk-whip 100 60           # publishes to streams load-0 .. load-99
#        just livewrk-whep 100 60 load-0    # subscribes to one already-published stream
#        just loadtest-channel throughput
# ============================================================

# WHIP publish load test; publishes streams load-0 .. load-(N-1)
[group('loadtest')]
livewrk-whip sessions="100" duration="60":
    cargo run --release --features=rsmpeg --bin livewrk -- whip \
        --whip {{server}}/whip/load --sessions {{sessions}} --duration {{duration}}

# Decode verification (--verify-window) requires building with --features=rsmpeg.
[group('loadtest')]
livewrk-whep sessions="100" duration="60" target_stream=stream:
    cargo run --release --features=rsmpeg --bin livewrk -- whep \
        --whep {{server}}/whep/{{target_stream}} --sessions {{sessions}} --duration {{duration}}

[group('loadtest')]
loadtest-channel mode="all":
    cargo run --release --features=source --bin datachannel_loadtest -- {{mode}}
