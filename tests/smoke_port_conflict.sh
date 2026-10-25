#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"
SERVER_PID=""

cleanup() {
    if [[ -n "$SERVER_PID" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
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

if grep -Eq '^POST |/whip/|/whep/' "$TMP_DIR/requests.log"; then
    echo "smoke-port-conflict: an occupied endpoint received a publish or subscribe request" >&2
    cat "$TMP_DIR/requests.log" >&2
    exit 1
fi
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "smoke-port-conflict: smoke cleanup killed the pre-existing HTTP service" >&2
    exit 1
fi

echo "smoke-port-conflict: both standalone checks refused the occupied endpoint without publishing"
