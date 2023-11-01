#!/bin/bash

TARGET_DIR="./target/release"

cat >stream.sdp <<EOF
v=0
m=video 5004 RTP/AVP 96
c=IN IP4 127.0.0.1
a=rtpmap:96 VP8/90000
EOF

cat >whip.sh <<EOF
#!/bin/bash
${TARGET_DIR}/whipinto -c vp8 -u http://localhost:3000/whip/777 --port 5003 --command "ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libvpx -cpu-used 5 -deadline 1 -g 10 -error-resilient 1 -auto-alt-ref 1 -f rtp 'rtp://127.0.0.1:{port}?pkt_size=1200'"
EOF

chmod +x whip.sh

cat >whep.sh <<EOF
#!/bin/bash
sleep 10
${TARGET_DIR}/whepfrom -c vp8 -u http://localhost:3000/whep/777 -t localhost:5004 --command "ffplay -protocol_whitelist rtp,file,udp -i stream.sdp"
EOF

chmod +x whep.sh

./multirun.sh \
    "${TARGET_DIR}/live777" \
    "./whip.sh" \
    "./whep.sh" \
    "sleep 30"

rm stream.sdp
rm whip.sh
rm whep.sh
