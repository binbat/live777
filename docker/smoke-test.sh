#!/bin/sh
# smoke-test — closed-loop live777 media check, shipped in the live777-all
# image as /usr/local/bin/smoke-test.
#
# Publishes a synthetic stream with whipsynth (WHIP) and verifies it end to
# end by decoding it back with whepprobe (WHEP).
#
# Default mode expects an already-running live777 at SMOKE_SERVER_URL and
# never touches it beyond publishing/subscribing. With --standalone it starts
# its own throwaway live777 first and stops only the processes it started.
#
# Env:
#   SMOKE_SERVER_URL     live777 base URL in server mode (default http://127.0.0.1:7777)
#   SMOKE_PORT           standalone HTTP port (default 7777)
#   SMOKE_TOKEN          Bearer token, passed as -t to both tools
#   SMOKE_STREAM         stream name (default smoke-<epoch>-<pid>)
#   SMOKE_ATTEMPTS       bounded probe attempts (default 5)
#   SMOKE_READY_TIMEOUT  seconds to wait for standalone live777 (default 30)
#
# Exit 0: success — stdout carries only the final whepprobe JSON.
# Exit 1: failure — the last probe JSON and tool logs go to stderr.
# Disable pathname expansion: TOKEN_OPT must undergo word splitting only.
set -uf

usage() {
    echo "usage: smoke-test [--standalone]" >&2
}

STANDALONE=0
for arg in "$@"; do
    case "$arg" in
        --standalone) STANDALONE=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "smoke-test: unknown argument: $arg" >&2
            usage
            exit 2
            ;;
    esac
done

PORT="${SMOKE_PORT:-7777}"
case "$PORT" in
    '' | *[!0-9]*)
        echo "smoke-test: SMOKE_PORT must be an integer from 1 to 65535" >&2
        exit 2
        ;;
esac
if [ "$PORT" -lt 1 ] || [ "$PORT" -gt 65535 ]; then
    echo "smoke-test: SMOKE_PORT must be an integer from 1 to 65535" >&2
    exit 2
fi
if [ "$STANDALONE" -eq 1 ]; then
    SERVER_URL="http://127.0.0.1:$PORT"
else
    SERVER_URL="${SMOKE_SERVER_URL:-http://127.0.0.1:$PORT}"
fi
STREAM="${SMOKE_STREAM:-smoke-$(date +%s)-$$}"
ATTEMPTS="${SMOKE_ATTEMPTS:-5}"
READY_TIMEOUT="${SMOKE_READY_TIMEOUT:-30}"

TOKEN_OPT=""
if [ -n "${SMOKE_TOKEN:-}" ]; then
    TOKEN_OPT="-t ${SMOKE_TOKEN}"
fi

LIVE777_PID=""
SYNTH_PID=""
if ! LOG_DIR="$(mktemp -d)"; then
    echo "smoke-test: failed to create a temporary log directory" >&2
    exit 1
fi

cleanup() {
    # Only processes this script started are stopped; a pre-existing live777
    # (default mode) is left running.
    [ -z "$SYNTH_PID" ] || kill "$SYNTH_PID" 2>/dev/null
    [ -z "$LIVE777_PID" ] || kill "$LIVE777_PID" 2>/dev/null
    [ -z "$SYNTH_PID" ] || wait "$SYNTH_PID" 2>/dev/null
    [ -z "$LIVE777_PID" ] || wait "$LIVE777_PID" 2>/dev/null
    rm -rf "$LOG_DIR"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

# Poll until the server answers HTTP (any status — auth failures still prove
# readiness); no fixed sleeps standing in for readiness.
wait_http_ready() {
    i=0
    while [ "$i" -lt "$2" ]; do
        if curl -s --max-time 2 -o /dev/null "$1/api/info"; then
            return 0
        fi
        i=$((i + 1))
        sleep 1
    done
    return 1
}

if [ "$STANDALONE" -eq 1 ]; then
    if curl -s --max-time 2 -o /dev/null "$SERVER_URL/api/info"; then
        echo "smoke-test: $SERVER_URL is already serving HTTP; refusing to start or publish standalone smoke" >&2
        exit 1
    fi
    cat >"$LOG_DIR/live777.toml" <<EOF
[http]
listen = "127.0.0.1:$PORT"

[rtsp]
listen = "127.0.0.1:0"

[webrtc]
ice_udp_addrs = ["127.0.0.1:0"]
EOF
    LIVE777_WEBRTC_ICE_UDP_ADDRS="127.0.0.1:0"
    export LIVE777_WEBRTC_ICE_UDP_ADDRS
    live777 -v -c "$LOG_DIR/live777.toml" >"$LOG_DIR/live777.log" 2>&1 &
    LIVE777_PID=$!
    echo "smoke-test: started live777 (pid $LIVE777_PID), waiting for $SERVER_URL" >&2
    i=0
    READY=0
    while [ "$i" -lt "$READY_TIMEOUT" ]; do
        if ! kill -0 "$LIVE777_PID" 2>/dev/null; then
            break
        fi
        if grep -q "Server listening on" "$LOG_DIR/live777.log" &&
            curl -s --max-time 2 -o /dev/null "$SERVER_URL/api/info"; then
            READY=1
            break
        fi
        i=$((i + 1))
        sleep 1
    done
    if [ "$READY" -ne 1 ]; then
        echo "smoke-test: own live777 did not become ready within ${READY_TIMEOUT}s; log:" >&2
        cat "$LOG_DIR/live777.log" >&2
        exit 1
    fi
elif ! wait_http_ready "$SERVER_URL" 5; then
    echo "smoke-test: no live777 answering at $SERVER_URL (start one, or rerun with --standalone)" >&2
    exit 1
fi

# Publish in the background. --ice-server "" is explicit but redundant: the
# tools default to no ICE servers (host candidates only), so the check stays
# off the internet. $TOKEN_OPT intentionally expands to two words.
# shellcheck disable=SC2086
whipsynth -v --whip "$SERVER_URL/whip/$STREAM" --acodec opus --ice-server "" $TOKEN_OPT \
    >"$LOG_DIR/whipsynth.log" 2>&1 &
SYNTH_PID=$!
echo "smoke-test: publishing stream '$STREAM' with whipsynth (pid $SYNTH_PID)" >&2

# Bounded retries: the WHEP subscribe also serves as publisher readiness —
# a fresh stream needs a moment before the first probe can connect.
attempt=1
LAST_JSON=""
while [ "$attempt" -le "$ATTEMPTS" ]; do
    # shellcheck disable=SC2086
    if LAST_JSON=$(whepprobe --whep "$SERVER_URL/whep/$STREAM" --output json \
        --decode-duration 1 --timeout 10 --ice-server "" $TOKEN_OPT); then
        echo "$LAST_JSON"
        exit 0
    fi
    attempt=$((attempt + 1))
    sleep 1
done

echo "smoke-test: probe failed after $ATTEMPTS attempts on $SERVER_URL/whep/$STREAM" >&2
if [ -n "$LAST_JSON" ]; then
    echo "smoke-test: last probe result:" >&2
    echo "$LAST_JSON" >&2
fi
echo "smoke-test: whipsynth log:" >&2
cat "$LOG_DIR/whipsynth.log" >&2
if [ "$STANDALONE" -eq 1 ]; then
    echo "smoke-test: live777 log:" >&2
    cat "$LOG_DIR/live777.log" >&2
fi
exit 1
