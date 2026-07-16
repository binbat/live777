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

# MacOS:
#   brew install gstreamer
# Debian:
#   apt install libgstreamer1.0-dev libgstrtspserver-1.0-dev
#
# Build some tools: test-rtsp-server
build-tools:
    gcc -o test-rtsp-server tools/test-rtsp-server.c $(pkg-config --cflags --libs gstreamer-1.0 gstreamer-rtsp-server-1.0)

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


# TODO: whipinto has some WARN
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


# Aa rtsp server receive stream
[group('simple-rtsp')]
ffmpeg-rtsp:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -vcodec libvpx -f rtsp rtsp://{{host}}:8550"

[group('simple-rtsp')]
ffmpeg-rtsp-tcp:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -vcodec libvpx -rtsp_transport tcp -f rtsp rtsp://{{host}}:8550"

[group('simple-rtsp')]
ffmpeg-rtsp-vp9:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -strict experimental -vcodec {{vp9}} -f rtsp rtsp://{{host}}:8550"

[group('simple-rtsp')]
ffmpeg-rtsp-h264:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{vsrc}} -vcodec {{h264}} -f rtsp rtsp://{{host}}:8550"

ffmpeg-rtsp-h264-raw:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{vsrc}} -vcodec libx264 -f rtsp rtsp://{{host}}:8550"

[group('simple-rtsp')]
ffmpeg-rtsp-h265:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/{{stream}} --command \
        "ffmpeg -re {{vsrc}} -vcodec {{h265}} -f rtsp rtsp://{{host}}:8550"

[group('simple-rtsp')]
ffplay-rtsp:
    cargo run --bin=whepfrom -- -o rtsp-listen://{{host}}:8650 -w {{server}}/whep/{{stream}} --command \
        "ffplay rtsp://{{host}}:8650"

[group('simple-rtsp')]
ffplay-rtsp-tcp:
    cargo run --bin=whepfrom -- -o rtsp-listen://{{host}}:8650 -w {{server}}/whep/{{stream}} --command \
        "ffplay rtsp://{{host}}:8650 -rtsp_transport tcp"


[group('cycle-rtsp')]
cycle-rtsp-0a:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8550 -w {{server}}/whip/cycle-rtsp-a --command \
        "ffmpeg -re {{asrc}} {{vsrc}} -acodec libopus -vcodec libvpx -f rtsp rtsp://{{host}}:8550"

[group('cycle-rtsp')]
cycle-rtsp-1a:
    cargo run --bin=whepfrom -- -o rtsp-listen://{{host}}:8650 -w {{server}}/whep/cycle-rtsp-a

[group('cycle-rtsp')]
cycle-rtsp-2b:
    cargo run --bin=whipinto -- -i rtsp://{{host}}:8650 -w {{server}}/whip/cycle-rtsp-b

[group('cycle-rtsp')]
cycle-rtsp-3c:
    cargo run --bin=whipinto -- -i rtsp-listen://{{host}}:8750 -w {{server}}/whip/cycle-rtsp-c

[group('cycle-rtsp')]
cycle-rtsp-4b:
    cargo run --bin=whepfrom -- -o rtsp://{{host}}:8750 -w {{server}}/whep/cycle-rtsp-b

[group('cycle-rtsp')]
cycle-rtsp-5c:
    cargo run --bin=whepfrom -- -o rtsp-listen://{{host}}:8850 -w {{server}}/whep/cycle-rtsp-c --command \
        "ffplay rtsp://{{host}}:8850"


# ============================================================
# ffmpeg push to liveion RTSP server (ANNOUNCE + RECORD)
# Usage: just ffmpeg-rtsp-push-h264
# ============================================================
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
[group('ffprobe-rtsp')]
ffprobe-rtsp:
    ffprobe -v error -hide_banner -i {{rtsps}}/{{stream}} -show_streams -of json

[group('ffprobe-rtsp')]
ffprobe-rtsp-tcp:
    ffprobe -rtsp_transport tcp -v error -hide_banner -i {{rtsps}}/{{stream}} -show_streams -of json

