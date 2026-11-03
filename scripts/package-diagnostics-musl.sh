#!/usr/bin/env bash
set -euo pipefail

MODE=${1:?usage: package-diagnostics-musl.sh package|verify|smoke TARGET VERSION}
TARGET=${2:?target is required}
VERSION=${3:?version is required}
DIST=${DIAGNOSTICS_DIST:-dist/diagnostics}
WORK=${DIAGNOSTICS_WORK:-/build/diagnostics-package}
BUILD_DIR=${CARGO_TARGET_DIR:-target}/${TARGET}/release
STAGE=live777-diagnostics-${VERSION}-${TARGET}
STAGE_DIR=${WORK}/${STAGE}
ARCHIVE=${DIST}/${STAGE}.tar.gz
VERIFY_DIR=${WORK}/verify
TOOLS=(whipsynth whepprobe livewrk)

sha256_file() {
    sha256sum "$1" | awk '{print $1}'
}

collect_static_libraries() {
    local destination=$1
    local maps=${LINK_MAP_DIR:-/build/link-maps}
    local map
    mkdir -p "${destination%/*}"
    : > "${destination}"
    for map in "${maps}/whipsynth.map" "${maps}/whepprobe.map" "${maps}/livewrk.map"; do
        [[ -s ${map} ]] || { echo "Missing linker map: ${map}" >&2; return 1; }
        awk '$1 == "LOAD" && $2 ~ /\.a$/ {print $2}' "${map}" >> "${destination}"
    done
    sort -u -o "${destination}" "${destination}"
    grep -q '/opt/ffmpeg/lib/libavcodec\.a$' "${destination}" || {
        echo "The linker maps do not contain static FFmpeg libraries" >&2
        return 1
    }
}

copy_notices() {
    local required
    mkdir -p "${STAGE_DIR}/licenses"
    cp -R licenses/diagnostics/. "${STAGE_DIR}/licenses/"
    for required in NOTICE.md GPL-2.0.txt LGPL-2.1.txt GPL-3.0.txt LGPL-3.0.txt Apache-2.0.txt FFmpeg-LICENSE.md; do
        [[ -s ${STAGE_DIR}/licenses/${required} ]] || { echo "Missing license notice: ${required}" >&2; return 1; }
    done
    cp LICENSE "${STAGE_DIR}/LICENSE"
}

package_musl() {
    local tool
    rm -rf "${STAGE_DIR}"
    mkdir -p "${STAGE_DIR}/bin" "${STAGE_DIR}/third-party" "${DIST}" "${WORK}"
    for tool in "${TOOLS[@]}"; do
        [[ -x ${BUILD_DIR}/${tool} ]] || { echo "Missing built tool: ${BUILD_DIR}/${tool}" >&2; return 1; }
        cp "${BUILD_DIR}/${tool}" "${STAGE_DIR}/bin/${tool}"
        chmod 0755 "${STAGE_DIR}/bin/${tool}"
    done
    copy_notices
    collect_static_libraries "${STAGE_DIR}/third-party/static-libraries.tsv"
    {
        printf 'target\t%s\n' "${TARGET}"
        printf 'version\t%s\n' "${VERSION}"
        printf 'ffmpeg\t8.1.2\n'
        printf 'linkage\tstatic\n'
    } > "${STAGE_DIR}/BUILD-INFO.tsv"
    rm -f "${ARCHIVE}"
    tar -czf "${ARCHIVE}" -C "${WORK}" "${STAGE}"
    [[ -s ${ARCHIVE} ]] || { echo "Binary diagnostics archive is empty" >&2; return 1; }
    printf 'archive=%s\n' "${ARCHIVE}"
    printf 'sha256=%s\n' "$(sha256_file "${ARCHIVE}")"
}

verify_static_elf() {
    local binary=$1
    local expected_machine
    case ${TARGET} in
        x86_64-unknown-linux-musl) expected_machine='Advanced Micro Devices X86-64' ;;
        aarch64-unknown-linux-musl) expected_machine='AArch64' ;;
        *) echo "Unsupported musl target: ${TARGET}" >&2; return 1 ;;
    esac
    readelf -hW "${binary}" | grep -q "Machine:.*${expected_machine}" || {
        echo "Wrong ELF architecture: ${binary}" >&2
        return 1
    }
    if readelf -lW "${binary}" | grep -q '[[:space:]]INTERP[[:space:]]'; then
        echo "Static binary has a PT_INTERP segment: ${binary}" >&2
        readelf -lW "${binary}" >&2
        return 1
    fi
    if readelf -dW "${binary}" 2>/dev/null | grep -q '(NEEDED)'; then
        echo "Static binary has dynamic dependencies: ${binary}" >&2
        readelf -dW "${binary}" >&2
        return 1
    fi
}

verify_musl() {
    local bundle tool required
    rm -rf "${VERIFY_DIR}"
    mkdir -p "${VERIFY_DIR}/home"
    tar -xzf "${ARCHIVE}" -C "${VERIFY_DIR}"
    mv "${VERIFY_DIR}/${STAGE}" "${VERIFY_DIR}/relocated bundle"
    bundle="${VERIFY_DIR}/relocated bundle"
    [[ ! -e ${bundle}/lib ]] || { echo "A static diagnostics bundle must not contain a lib directory" >&2; return 1; }
    for required in NOTICE.md GPL-2.0.txt LGPL-2.1.txt GPL-3.0.txt LGPL-3.0.txt Apache-2.0.txt FFmpeg-LICENSE.md; do
        [[ -s ${bundle}/licenses/${required} ]] || { echo "Missing bundled license: ${required}" >&2; return 1; }
    done
    [[ -s ${bundle}/third-party/static-libraries.tsv ]] || { echo "Missing static-link provenance" >&2; return 1; }
    for tool in "${TOOLS[@]}"; do
        [[ -f ${bundle}/bin/${tool} && -x ${bundle}/bin/${tool} ]] || {
            echo "Missing direct static executable: ${tool}" >&2
            return 1
        }
        [[ ! -e ${bundle}/bin/${tool}.real ]] || { echo "Unexpected launcher payload: ${tool}.real" >&2; return 1; }
        verify_static_elf "${bundle}/bin/${tool}"
        env -i PATH=/usr/bin:/bin HOME="${VERIFY_DIR}/home" TMPDIR="${VERIFY_DIR}" \
            "${bundle}/bin/${tool}" --version
    done
    verify_static_elf "${BUILD_DIR}/live777"
}

stop_pid() {
    local pid=${1:-}
    local watchdog
    [[ -n ${pid} ]] || return 0
    kill "${pid}" 2>/dev/null || true
    (
        sleep 5
        kill -KILL "${pid}" 2>/dev/null || true
    ) &
    watchdog=$!
    wait "${pid}" 2>/dev/null || true
    kill "${watchdog}" 2>/dev/null || true
    wait "${watchdog}" 2>/dev/null || true
}

wait_http_ready() {
    local url=$1
    local attempt
    for attempt in $(seq 1 30); do
        if curl -fsS --max-time 2 -o /dev/null "${url}/api/info"; then
            return 0
        fi
        sleep 1
    done
    return 1
}

verify_livewrk_worker() {
    local bundle=$1
    local port=17778
    local stream=livewrk-musl-smoke
    local temporary server_pid='' synth_pid=''
    temporary=$(mktemp -d)
    cat > "${temporary}/live777.toml" <<EOF
[http]
listen = "127.0.0.1:${port}"

[rtsp]
listen = "127.0.0.1:0"

[webrtc]
ice_udp_addrs = ["127.0.0.1:0"]
EOF
    cleanup_worker() {
        stop_pid "${synth_pid}"
        stop_pid "${server_pid}"
        rm -rf "${temporary}"
    }
    trap cleanup_worker EXIT
    LIVE777_WEBRTC_ICE_UDP_ADDRS=127.0.0.1:0 live777 -v -c "${temporary}/live777.toml" \
        > "${temporary}/live777.log" 2>&1 &
    server_pid=$!
    wait_http_ready "http://127.0.0.1:${port}" || {
        cat "${temporary}/live777.log" >&2
        return 1
    }
    whipsynth -v --whip "http://127.0.0.1:${port}/whip/${stream}" --acodec opus --ice-server '' \
        > "${temporary}/whipsynth.log" 2>&1 &
    synth_pid=$!
    if ! livewrk whep --whep "http://127.0.0.1:${port}/whep/${stream}" \
        --sessions 1 --duration 12 --verify-window 2 --ice-server '' \
        --min-connected 1 --fail-on-any-session --output json > "${temporary}/livewrk.json" \
        2> "${temporary}/livewrk.log"; then
        cat "${temporary}/livewrk.log" >&2
        cat "${temporary}/livewrk.json" >&2
        cat "${temporary}/whipsynth.log" >&2
        return 1
    fi
    cat "${temporary}/livewrk.json"
    grep -Eq '"success"[[:space:]]*:[[:space:]]*true' "${temporary}/livewrk.json" || {
        echo "livewrk worker verification was unsuccessful" >&2
        return 1
    }
    grep -Eq '"windows_total"[[:space:]]*:[[:space:]]*[1-9][0-9]*' "${temporary}/livewrk.json" || {
        echo "livewrk completed no decode-worker windows" >&2
        return 1
    }
    grep -Eq '"frames_decoded"[[:space:]]*:[[:space:]]*[1-9][0-9]*' "${temporary}/livewrk.json" || {
        echo "livewrk decode worker decoded no frames" >&2
        return 1
    }
    cleanup_worker
    trap - EXIT
}

smoke_musl() {
    local bundle="${VERIFY_DIR}/relocated bundle"
    [[ -x ${bundle}/bin/whipsynth && -x ${bundle}/bin/whepprobe && -x ${bundle}/bin/livewrk ]] || {
        echo "Run the verify mode before smoke" >&2
        return 1
    }
    cp "${BUILD_DIR}/live777" "${bundle}/bin/live777"
    chmod 0755 "${bundle}/bin/live777"
    export PATH="${bundle}/bin:${PATH}"
    SMOKE_PORT=17777 docker/smoke-test.sh --standalone
    verify_livewrk_worker "${bundle}"
}

case ${MODE} in
    package) package_musl ;;
    verify) verify_musl ;;
    smoke) smoke_musl ;;
    *) echo "Unknown mode: ${MODE}" >&2; exit 2 ;;
esac
