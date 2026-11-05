#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"
SERVER_PID=""
RTP_PID=""

cleanup() {
    for pid in "$RTP_PID" "$SERVER_PID"; do
        if [[ -n "$pid" ]]; then
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

python3 - "$TMP_DIR/port" "$TMP_DIR/requests.log" <<'PY' &
import http.server
import pathlib
import sys

port_file = pathlib.Path(sys.argv[1])
request_log = pathlib.Path(sys.argv[2])


class Handler(http.server.BaseHTTPRequestHandler):
    def reply(self):
        with request_log.open("a", encoding="utf-8") as log:
            log.write(f"{self.command} {self.path}\n")
        self.send_response(200)
        self.send_header("Content-Length", "2")
        self.end_headers()
        self.wfile.write(b"{}")

    do_GET = reply
    do_POST = reply

    def log_message(self, *args):
        pass


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
port_file.write_text(str(server.server_port), encoding="utf-8")
server.serve_forever()
PY
SERVER_PID=$!

for ((i = 0; i < 50; i++)); do
    [[ -s "$TMP_DIR/port" ]] && break
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "smoke-port-conflict: test HTTP server exited" >&2
        exit 1
    fi
    sleep 0.1
done
PORT="$(cat "$TMP_DIR/port")"

if SMOKE_PORT="$PORT" "$ROOT_DIR/docker/smoke-test.sh" --standalone \
    >"$TMP_DIR/docker-smoke.log" 2>&1; then
    echo "smoke-port-conflict: docker smoke unexpectedly succeeded" >&2
    exit 1
fi
if ! grep -q "already serving HTTP" "$TMP_DIR/docker-smoke.log"; then
    echo "smoke-port-conflict: docker smoke did not report the occupied endpoint" >&2
    cat "$TMP_DIR/docker-smoke.log" >&2
    exit 1
fi

if SMOKE_BRIDGE_SERVER_PORT="$PORT" LIVE777_BIN=true WHIPINTO_BIN=true WHEPFROM_BIN=true \
    "$ROOT_DIR/scripts/smoke-bridge.sh" >"$TMP_DIR/bridge-smoke.log" 2>&1; then
    echo "smoke-port-conflict: bridge smoke unexpectedly succeeded" >&2
    exit 1
fi
if ! grep -q "already serving HTTP" "$TMP_DIR/bridge-smoke.log"; then
    echo "smoke-port-conflict: bridge smoke did not report the occupied endpoint" >&2
    cat "$TMP_DIR/bridge-smoke.log" >&2
    exit 1
fi

python3 - "$TMP_DIR/rtp-port" <<'PY' &
import pathlib
import socket
import sys

port_file = pathlib.Path(sys.argv[1])
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(("127.0.0.1", 0))
port_file.write_text(str(sock.getsockname()[1]), encoding="utf-8")
while True:
    sock.recvfrom(65535)
PY
RTP_PID=$!

for ((i = 0; i < 50; i++)); do
    [[ -s "$TMP_DIR/rtp-port" ]] && break
    if ! kill -0 "$RTP_PID" 2>/dev/null; then
        echo "smoke-port-conflict: test RTP service exited" >&2
        exit 1
    fi
    sleep 0.1
done
if [[ ! -s "$TMP_DIR/rtp-port" ]]; then
    echo "smoke-port-conflict: test RTP service did not report its port" >&2
    exit 1
fi
RTP_PORT="$(cat "$TMP_DIR/rtp-port")"
FREE_SERVER_PORT="$(python3 - <<'PY'
import socket

with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"

if SMOKE_BRIDGE_SERVER_PORT="$FREE_SERVER_PORT" SMOKE_BRIDGE_RTP_PORT="$RTP_PORT" \
    LIVE777_BIN=true WHIPINTO_BIN=true WHEPFROM_BIN=true \
    "$ROOT_DIR/scripts/smoke-bridge.sh" >"$TMP_DIR/bridge-rtp-smoke.log" 2>&1; then
    echo "smoke-port-conflict: bridge smoke unexpectedly accepted an occupied RTP port" >&2
    exit 1
fi
if ! grep -q "explicit RTP UDP port $RTP_PORT is already in use" "$TMP_DIR/bridge-rtp-smoke.log"; then
    echo "smoke-port-conflict: bridge smoke did not report the occupied RTP endpoint" >&2
    cat "$TMP_DIR/bridge-rtp-smoke.log" >&2
    exit 1
fi

mkdir -p "$TMP_DIR/order-bin"
cat >"$TMP_DIR/order-bin/curl" <<'SH'
#!/usr/bin/env bash
[[ -f "$ORDER_DIR/live777-started" ]]
SH
cat >"$TMP_DIR/order-live777.sh" <<'SH'
#!/usr/bin/env bash
echo "Server listening on" >&2
touch "$ORDER_DIR/live777-started"
trap 'exit 0' TERM
while :; do
    sleep 1
done
SH
cat >"$TMP_DIR/order-publisher.sh" <<'SH'
#!/usr/bin/env bash
trap 'exit 0' TERM
while :; do
    sleep 1
done
SH
cat >"$TMP_DIR/order-whepfrom.sh" <<'SH'
#!/usr/bin/env bash
output=""
while (($#)); do
    case "$1" in
        -o)
            output="$2"
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done
python3 - "${output##*:}" "$ORDER_DIR/receiver-state" <<'PY'
import errno
import pathlib
import socket
import sys

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
state_file = pathlib.Path(sys.argv[2])
try:
    sock.bind(("127.0.0.1", int(sys.argv[1])))
except OSError as error:
    if error.errno != errno.EADDRINUSE:
        raise
    state_file.write_text("bound", encoding="utf-8")
else:
    state_file.write_text("unbound", encoding="utf-8")
PY
SH
chmod +x "$TMP_DIR/order-bin/curl" "$TMP_DIR/order-live777.sh" \
    "$TMP_DIR/order-publisher.sh" "$TMP_DIR/order-whepfrom.sh"

if ORDER_DIR="$TMP_DIR" PATH="$TMP_DIR/order-bin:$PATH" \
    SMOKE_BRIDGE_SERVER_PORT="$FREE_SERVER_PORT" SMOKE_BRIDGE_PROBE_TIMEOUT=2 \
    LIVE777_BIN="$TMP_DIR/order-live777.sh" WHIPINTO_BIN="$TMP_DIR/order-publisher.sh" \
    WHEPFROM_BIN="$TMP_DIR/order-whepfrom.sh" \
    "$ROOT_DIR/scripts/smoke-bridge.sh" >"$TMP_DIR/bridge-order-smoke.log" 2>&1; then
    echo "smoke-port-conflict: bridge smoke unexpectedly succeeded with a stub subscriber" >&2
    exit 1
fi
if [[ ! -f "$TMP_DIR/receiver-state" ]] || [[ "$(cat "$TMP_DIR/receiver-state")" != "bound" ]]; then
    echo "smoke-port-conflict: RTP receiver was not bound before whepfrom started" >&2
    cat "$TMP_DIR/bridge-order-smoke.log" >&2
    exit 1
fi

if grep -Eq '^POST |/whip/|/whep/' "$TMP_DIR/requests.log"; then
    echo "smoke-port-conflict: an occupied endpoint received a publish or subscribe request" >&2
    cat "$TMP_DIR/requests.log" >&2
    exit 1
fi
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "smoke-port-conflict: smoke cleanup killed the pre-existing HTTP service" >&2
    exit 1
fi
if ! kill -0 "$RTP_PID" 2>/dev/null; then
    echo "smoke-port-conflict: smoke cleanup killed the pre-existing RTP service" >&2
    exit 1
fi

echo "smoke-port-conflict: occupied endpoints were refused and the RTP receiver stayed bound before whepfrom"
