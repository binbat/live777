#!/usr/bin/env bash
# Real whipinto synth -> live777 -> whepfrom RTP/SDP -> ffprobe decode check.
# Required tools: live777, whipinto (with rsmpeg), whepfrom, curl and ffprobe.
# Override binary paths with LIVE777_BIN, WHIPINTO_BIN and WHEPFROM_BIN.
# SMOKE_TOKEN is optional; the standalone server itself is open.
set -Eeuo pipefail

SERVER_PORT="${SMOKE_BRIDGE_SERVER_PORT:-17777}"
RTP_PORT="${SMOKE_BRIDGE_RTP_PORT:-}"
STREAM="${SMOKE_BRIDGE_STREAM:-smoke-bridge-$(date +%s)-$$}"
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
if [[ -n "$RTP_PORT" ]]; then
    validate_port SMOKE_BRIDGE_RTP_PORT "$RTP_PORT"
fi
if [[ ! "$READY_TIMEOUT" =~ ^[1-9][0-9]*$ ]]; then
    echo "smoke-bridge: SMOKE_BRIDGE_READY_TIMEOUT must be a positive integer" >&2
    exit 2
fi
if [[ ! "$PROBE_TIMEOUT" =~ ^[1-9][0-9]*$ ]]; then
    echo "smoke-bridge: SMOKE_BRIDGE_PROBE_TIMEOUT must be a positive integer" >&2
    exit 2
fi

for tool in "$LIVE777_BIN" "$WHIPINTO_BIN" "$WHEPFROM_BIN" curl ffprobe; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "smoke-bridge: required tool not found: $tool" >&2
        exit 1
    fi
done

TMP_DIR="$(mktemp -d)"
RECEIVER_SDP="$TMP_DIR/receiver.sdp"
RECEIVER_JSON="$TMP_DIR/receiver.json"
FFPROBE_LOG="$TMP_DIR/ffprobe.log"
SDP_FILE="$TMP_DIR/output.sdp"
LIVE777_PID=""
PUBLISHER_PID=""
SUBSCRIBER_PID=""
RECEIVER_PID=""

stop_pid() {
    local pid="$1"
    local watchdog
    if [[ -n "$pid" ]]; then
        kill -TERM "$pid" 2>/dev/null || true
        (
            sleep 5
            kill -KILL "$pid" 2>/dev/null || true
        ) &
        watchdog=$!
        wait "$pid" 2>/dev/null || true
        kill "$watchdog" 2>/dev/null || true
        wait "$watchdog" 2>/dev/null || true
    fi
}

cleanup() {
    stop_pid "$SUBSCRIBER_PID"
    stop_pid "$PUBLISHER_PID"
    stop_pid "$LIVE777_PID"
    stop_pid "$RECEIVER_PID"
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

pid_running() {
    [[ -n "$1" ]] && jobs -pr | grep -q "^$1$"
}

start_receiver() {
    local port="$1"
    local sockets
    cat >"$RECEIVER_SDP" <<EOF
v=0
o=- 0 0 IN IP4 127.0.0.1
s=live777-smoke-bridge
c=IN IP4 127.0.0.1
t=0 0
m=video $port RTP/AVP 102
a=rtpmap:102 H264/90000
a=fmtp:102 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f
a=recvonly
EOF
    : >"$RECEIVER_JSON"
    : >"$FFPROBE_LOG"
    ffprobe -v debug -protocol_whitelist rtp,file,udp \
        -read_intervals '%+#30' -show_frames -show_streams \
        -show_entries frame=media_type,width,height:stream=codec_name \
        -of json -i "$RECEIVER_SDP" >"$RECEIVER_JSON" 2>"$FFPROBE_LOG" &
    RECEIVER_PID=$!

    for ((i = 0; i < 100; i++)); do
        sockets=$(grep -c "end receive buffer size reported" "$FFPROBE_LOG" || true)
        if ((sockets >= 2)); then
            return 0
        fi
        if ! pid_running "$RECEIVER_PID"; then
            wait "$RECEIVER_PID" 2>/dev/null || true
            RECEIVER_PID=""
            if grep -q "Address already in use" "$FFPROBE_LOG"; then
                return 1
            fi
            return 2
        fi
        sleep 0.05
    done
    return 2
}

if [[ -n "$RTP_PORT" ]]; then
    if start_receiver "$RTP_PORT"; then
        :
    else
        status=$?
        if ((status == 1)); then
            echo "smoke-bridge: explicit RTP UDP port $RTP_PORT is already in use" >&2
        else
            echo "smoke-bridge: could not start the receiver on explicit RTP UDP port $RTP_PORT; log:" >&2
            cat "$FFPROBE_LOG" >&2
        fi
        exit 1
    fi
else
    for ((i = 0; i < 50; i++)); do
        candidate=$((20000 + 2 * (RANDOM % 20000)))
        if start_receiver "$candidate"; then
            RTP_PORT="$candidate"
            break
        else
            status=$?
        fi
        if ((status == 2)); then
            echo "smoke-bridge: could not start the receiver on candidate RTP UDP port $candidate; log:" >&2
            cat "$FFPROBE_LOG" >&2
            exit 1
        fi
    done
    if [[ -z "$RTP_PORT" ]]; then
        echo "smoke-bridge: could not allocate and bind an available RTP UDP port" >&2
        exit 1
    fi
fi

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
    if ! pid_running "$LIVE777_PID"; then
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

generated_sdp_valid() {
    grep -q "^m=video $RTP_PORT RTP/AVP 102" "$SDP_FILE" &&
        grep -q "^a=rtpmap:102 H264/90000" "$SDP_FILE" &&
        grep -q "^c=IN IP4 127.0.0.1" "$SDP_FILE"
}

failure=""
for ((i = 0; i < PROBE_TIMEOUT; i++)); do
    if ! pid_running "$PUBLISHER_PID"; then
        failure="smoke-bridge: whipinto exited before decoding succeeded"
        break
    fi
    if ! pid_running "$SUBSCRIBER_PID"; then
        failure="smoke-bridge: whepfrom exited before decoding succeeded"
        break
    fi
    if ! pid_running "$RECEIVER_PID"; then
        break
    fi
    sleep 1
done

LAST_JSON=""
if [[ -z "$failure" ]]; then
    if pid_running "$RECEIVER_PID"; then
        if [[ -s "$SDP_FILE" ]] && ! generated_sdp_valid; then
            failure="smoke-bridge: whepfrom SDP does not match the bound H264 receiver"
        else
            failure="smoke-bridge: timed out after ${PROBE_TIMEOUT}s waiting for decoded video"
        fi
    else
        if wait "$RECEIVER_PID"; then
            receiver_status=0
        else
            receiver_status=$?
        fi
        RECEIVER_PID=""
        if ((receiver_status != 0)); then
            failure="smoke-bridge: ffprobe receiver exited with status $receiver_status"
        elif [[ ! -s "$SDP_FILE" ]]; then
            failure="smoke-bridge: whepfrom did not generate its SDP file"
        elif ! generated_sdp_valid; then
            failure="smoke-bridge: whepfrom SDP does not match the bound H264 receiver"
        else
            LAST_JSON="$(cat "$RECEIVER_JSON")"
            if grep -q '"media_type": "video"' "$RECEIVER_JSON" &&
                grep -q '"codec_name": "h264"' "$RECEIVER_JSON" &&
                grep -q '"width": 320' "$RECEIVER_JSON" &&
                grep -q '"height": 240' "$RECEIVER_JSON"; then
                echo "$LAST_JSON"
                exit 0
            fi
            failure="smoke-bridge: no decoded h264 320x240 video"
        fi
    fi
fi

echo "$failure" >&2
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
