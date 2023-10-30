#!/bin/bash

cargo build --release
cargo build --package=whipinto --release
cargo build --package=whepfrom --release


./multirun.sh  \
'./live777' 'whipinto -c vp8 -u http://localhost:3000/whip/777 --port 5003' \
"ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5003?pkt_size=1200'"  \
"whepfrom -c vp8 -u http://localhost:3000/whep/777 -t localhost:5004" \
"ffplay -protocol_whitelist rtp,file,udp -i stream.sdp"
