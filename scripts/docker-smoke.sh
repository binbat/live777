#!/usr/bin/env bash
set -Eeuo pipefail

APP=${1:?usage: docker-smoke.sh APP IMAGE}
IMAGE=${2:?image is required}
CONTAINER=""

cleanup() {
    status=$?
    if [[ -n ${CONTAINER} ]]; then
        if ((status != 0)); then
            docker logs "${CONTAINER}" || true
        fi
        docker stop "${CONTAINER}" >/dev/null 2>&1 || true
    fi
    exit "${status}"
}
trap cleanup EXIT

run() {
    docker run --rm --network host "${IMAGE}" "$@"
}

case ${APP} in
    live777-all)
        run smoke-test --standalone
        ;;
    liveion | live777-server)
        CONTAINER="docker-smoke-${APP}-$$"
        docker run --rm --network host -d --name "${CONTAINER}" "${IMAGE}" >/dev/null
        if [[ ${APP} == live777-server ]]; then
            run liveman --version
        fi
        for ((i = 0; i < 30; i++)); do
            if curl -fsS --max-time 2 http://127.0.0.1:7777/api/info >/dev/null; then
                exit 0
            fi
            if [[ $(docker inspect -f '{{.State.Running}}' "${CONTAINER}") != true ]]; then
                echo "${APP}: container exited before becoming ready" >&2
                exit 1
            fi
            sleep 1
        done
        echo "${APP}: API did not become ready within 30 seconds" >&2
        exit 1
        ;;
    ffmpeg)
        run ffmpeg -version
        run ffmpeg -hide_banner -h muxer=whip >/dev/null
        ;;
    gstreamer)
        run gst-launch-1.0 --version
        run gst-inspect-1.0 whipsink
        run gst-inspect-1.0 whepsrc
        ;;
    live777-client)
        run whipinto --version
        run whepfrom --version
        run ffmpeg -hide_banner -h muxer=whip >/dev/null
        ;;
    liveman)
        run liveman --version
        ;;
    net4mqtt | whipinto | whepfrom)
        run "${APP}" --version
        ;;
    *)
        echo "Unsupported Docker smoke app: ${APP}" >&2
        exit 2
        ;;
esac
