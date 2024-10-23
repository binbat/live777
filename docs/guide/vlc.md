# VLC

VLC RTP stream

**Note: VLC can't support all video codec**

```
vlc -> whipinto -> live777 -> whepfrom -> vlc
```

## Video: VP8

generates a video

```bash
ffmpeg -f lavfi -i testsrc=size=640x480:rate=30:d=30 \
-c:v libvpx output.webm
```

use this video send rtp

```bash
vlc -vvv output.webm --loop --sout '#rtp{dst=127.0.0.1,port=5003}'
```

```bash
cat > stream.sdp << EOF
v=0
m=video 5004 RTP/AVP 96
c=IN IP4 127.0.0.1
a=rtpmap:96 VP8/90000
EOF
```

Use VLC player

```bash
vlc stream.sdp
```

## Video: H264

```bash
ffmpeg -f lavfi -i testsrc=size=640x480:rate=30:d=30 \
-c:v libx264 \
-x264-params "level-asymmetry-allowed=1:packetization-mode=1:profile-level-id=42001f" \
output.mp4
```

use this video send rtp

```bash
vlc -vvv output.mp4 --loop --sout '#rtp{dst=127.0.0.1,port=5003}'
```

```bash
cat > stream.sdp << EOF
v=0
c=IN IP4 127.0.0.1
a=recvonly
a=type:broadcast
a=charset:UTF-8
m=video 5003 RTP/AVP 96
b=AS:43
b=RR:0
a=rtpmap:96 H264/90000
a=fmtp:96 packetization-mode=1;profile-level-id=f4001e;sprop-parameter-sets=Z/QAHpGbKBQHtgIgAAADACAAAAeB4sWywA==,aOvjxEhE;
a=rtcp:5004
EOF
```

```bash
vlc stream.sdp
```

## Audio: Opus

```bash
ffmpeg -f lavfi -i sine=frequency=1000:duration=30 \
-acodec libopus output.opus
```

```bash
vlc -vvv output.opus --loop --sout '#rtp{dst=127.0.0.1,port=5003}'
```

```bash
cat > stream.sdp << EOF
v=0
c=IN IP4 127.0.0.1
a=recvonly
a=type:broadcast
a=charset:UTF-8
m=audio 5003 RTP/AVP 96
b=RR:0
a=rtpmap:96 opus/48000/2
a=rtcp:5004
EOF
```

```bash
vlc stream.sdp
```

