#!/usr/bin/env bash
# Real whipinto synth -> live777 -> whepfrom RTP/SDP -> ffprobe decode check.
# Required tools: live777, whipinto (with rsmpeg), whepfrom, curl, ffprobe,
# and timeout. Override binary paths with LIVE777_BIN, WHIPINTO_BIN and
# WHEPFROM_BIN. SMOKE_TOKEN is optional; the standalone server itself is open.
set -Eeuo pipefail

SERVER_PORT="${SMOKE_BRIDGE_SERVER_PORT:-17777}"
RTP_PORT="${SMOKE_BRIDGE_RTP_PORT:-19000}"
STREAM="${SMOKE_BRIDGE_STREAM:-smoke-bridge-$(date +%s)-$$}"
ATTEMPTS="${SMOKE_BRIDGE_ATTEMPTS:-5}"
READY_TIMEOUT="${SMOKE_BRIDGE_READY_TIMEOUT:-30}"
PROBE_TIMEOUT="${SMOKE_BRIDGE_PROBE_TIMEOUT:-15}"
LIVE777_BIN="${LIVE777_BIN:-live777}"
WHIPINTO_BIN="${WHIPINTO_BIN:-whipinto}"
WHEPFROM_BIN="${WHEPFROM_BIN:-whepfrom}"

validate_port() {
    local name="$1"
    local value="$2"
    if [[ ! "$value" =~ ^[0-9]+$ ]] || ((value < 1 || value > 65535)); then
        echo "smoke-bridge: $name must be an integer from 1 to 65535" >&2
        exit 2
    fi
}

validate_port SMOKE_BRIDGE_SERVER_PORT "$SERVER_PORT"
validate_port SMOKE_BRIDGE_RTP_PORT "$RTP_PORT"
if [[ ! "$ATTEMPTS" =~ ^[1-9][0-9]*$ ]]; then
    echo "smoke-bridge: SMOKE_BRIDGE_ATTEMPTS must be a positive integer" >&2
    exit 2
fi

for tool in "$LIVE777_BIN" "$WHIPINTO_BIN" "$WHEPFROM_BIN" curl ffprobe timeout; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "smoke-bridge: required tool not found: $tool" >&2
        exit 1
    fi
done

TMP_DIR="$(mktemp -d)"
LIVE777_PID=""
PUBLISHER_PID=""
SUBSCRIBER_PID=""

stop_pid() {
    local pid="$1"
    if [[ -n "$pid" ]]; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
}

cleanup() {
    stop_pid "$SUBSCRIBER_PID"
    stop_pid "$PUBLISHER_PID"
    stop_pid "$LIVE777_PID"
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

cat >"$TMP_DIR/live777.toml" <<EOF
[http]
listen = "127.0.0.1:$SERVER_PORT"

[rtsp]
listen = "127.0.0.1:0"

[webrtc]
ice_udp_addrs = ["127.0.0.1:0"]
EOF

export LIVE777_WEBRTC_ICE_UDP_ADDRS="127.0.0.1:0"
SERVER_URL="http://127.0.0.1:$SERVER_PORT"
SDP_FILE="$TMP_DIR/output.sdp"
TOKEN_ARGS=()
if [[ -n "${SMOKE_TOKEN:-}" ]]; then
    TOKEN_ARGS=(-t "$SMOKE_TOKEN")
fi

if curl -s --max-time 2 -o /dev/null "$SERVER_URL/api/info"; then
    echo "smoke-bridge: $SERVER_URL is already serving HTTP; refusing to start or publish standalone smoke" >&2
    exit 1
fi

"$LIVE777_BIN" -v -c "$TMP_DIR/live777.toml" >"$TMP_DIR/live777.log" 2>&1 &
LIVE777_PID=$!

ready=0
for ((i = 0; i < READY_TIMEOUT; i++)); do
    if ! kill -0 "$LIVE777_PID" 2>/dev/null; then
        break
    fi
    if grep -q "Server listening on" "$TMP_DIR/live777.log" &&
        curl -s --max-time 2 -o /dev/null "$SERVER_URL/api/info"; then
        ready=1
        break
    fi
    sleep 1
done
if ((ready == 0)); then
    echo "smoke-bridge: own live777 did not become ready at $SERVER_URL; log:" >&2
    cat "$TMP_DIR/live777.log" >&2
    exit 1
fi

"$WHIPINTO_BIN" -v --ice-server "" "${TOKEN_ARGS[@]}" \
    -i 'synth://h264?width=320&height=240&fps=15' \
    -w "$SERVER_URL/whip/$STREAM" >"$TMP_DIR/whipinto.log" 2>&1 &
PUBLISHER_PID=$!

"$WHEPFROM_BIN" -v --ice-server "" "${TOKEN_ARGS[@]}" \
    -w "$SERVER_URL/whep/$STREAM" \
    -o "rtp://127.0.0.1:$RTP_PORT" --sdp-file "$SDP_FILE" \
    >"$TMP_DIR/whepfrom.log" 2>&1 &
SUBSCRIBER_PID=$!

LAST_JSON=""
for ((attempt = 1; attempt <= ATTEMPTS; attempt++)); do
    if ! kill -0 "$PUBLISHER_PID" 2>/dev/null; then
        echo "smoke-bridge: whipinto exited before decoding succeeded" >&2
        break
    fi
    if ! kill -0 "$SUBSCRIBER_PID" 2>/dev/null; then
        echo "smoke-bridge: whepfrom exited before decoding succeeded" >&2
        break
    fi
    if [[ -s "$SDP_FILE" ]] && LAST_JSON=$(timeout "$PROBE_TIMEOUT" \
        ffprobe -v error -protocol_whitelist rtp,file,udp \
        -read_intervals '%+#2' -show_frames \
        -show_entries frame=media_type,codec_name,width,height \
        -of json -i "$SDP_FILE" 2>"$TMP_DIR/ffprobe.log") &&
        grep -q '"media_type": "video"' <<<"$LAST_JSON"; then
        echo "$LAST_JSON"
        exit 0
    fi
    sleep 1
done

echo "smoke-bridge: no decoded video after $ATTEMPTS attempts" >&2
if [[ -n "$LAST_JSON" ]]; then
    echo "$LAST_JSON" >&2
fi
for log in ffprobe whepfrom whipinto live777; do
    if [[ -f "$TMP_DIR/$log.log" ]]; then
        echo "smoke-bridge: $log log:" >&2
        cat "$TMP_DIR/$log.log" >&2
    fi
done
exit 1
