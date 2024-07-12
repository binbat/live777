# VLC

VLC RTP stream

**Note: VLC can't support all video codec**

```bash
vlc -vvv <INPUT_FILE> --sout '#transcode{vcodec=h264}:rtp{dst=127.0.0.1,port=5003}'
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

