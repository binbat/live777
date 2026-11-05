#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"
PIDS=()

cleanup() {
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

mkdir "$TMP_DIR/bin"
cat >"$TMP_DIR/bin/curl" <<'EOF'
#!/bin/sh
[ -f "${FAKE_LIVE777_READY:?}" ]
EOF
cat >"$TMP_DIR/bin/live777" <<'EOF'
#!/bin/sh
touch "${FAKE_LIVE777_READY:?}"
echo "Server listening on"
while :; do sleep 1; done
EOF
cat >"$TMP_DIR/bin/whipsynth" <<'EOF'
#!/bin/sh
while :; do sleep 1; done
EOF
cat >"$TMP_DIR/bin/whepprobe" <<'EOF'
#!/bin/sh
cat <<'JSON'
{
  "success": true,
  "connected": true,
  "backend": "rsmpeg",
  "video_codec": "vp8",
  "audio_codec": null,
  "audio_frame_count": 0,
  "audio_sample_rate": 0,
  "audio_channels": 0,
  "width": 640,
  "height": 480,
  "frame_count": 30,
  "duration_ms": 1000,
  "video_tracks": 1,
  "audio_tracks": 0,
  "video_bytes_received": 25000,
  "audio_bytes_received": 0,
  "error": null
}
JSON
EOF
chmod +x "$TMP_DIR/bin/"*
export FAKE_LIVE777_READY="$TMP_DIR/live777-ready"
export PATH="$TMP_DIR/bin:$PATH"

set +e
SMOKE_ATTEMPTS=1 SMOKE_READY_TIMEOUT=2 \
    timeout 15 "$ROOT_DIR/docker/smoke-test.sh" --standalone \
    >"$TMP_DIR/smoke.log" 2>&1
status=$?
set -e
if ((status == 0)); then
    echo "smoke-audio-required: video-only probe unexpectedly succeeded" >&2
    exit 1
fi
if ((status == 124)); then
    echo "smoke-audio-required: smoke test timed out" >&2
    exit 1
fi
if ! grep -q "publishing stream" "$TMP_DIR/smoke.log"; then
    echo "smoke-audio-required: test did not reach the fake publisher" >&2
    cat "$TMP_DIR/smoke.log" >&2
    exit 1
fi
if ! grep -q "lacked decoded opus audio" "$TMP_DIR/smoke.log"; then
    echo "smoke-audio-required: missing audio was not diagnosed" >&2
    cat "$TMP_DIR/smoke.log" >&2
    exit 1
fi

echo "smoke-audio-required: video-only probe was rejected"
