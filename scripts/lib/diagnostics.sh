#!/usr/bin/env bash
# Shared helpers for scripts/package-diagnostics*.sh.
#
# Sourced by the packaging scripts; not meant to be executed directly. Every
# script still owns its own mode dispatch and archive layout, because the
# dynamic (glibc/macOS) and static (musl) bundles disagree on linkage,
# provenance and rebuild instructions.

TOOLS=(whipsynth whepprobe livewrk)

DIAGNOSTICS_NOTICES=(
    NOTICE.md
    GPL-2.0.txt
    LGPL-2.1.txt
    GPL-3.0.txt
    LGPL-3.0.txt
    Apache-2.0.txt
    FFmpeg-LICENSE.md
)

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

manifest_value() {
    awk -F '\t' -v key="$2" '$1 == key {sub(/^[^\t]*\t/, ""); print; exit}' "$1"
}

# Copy the bundled third-party notices and fail if any required file is absent.
copy_diagnostics_notices() {
    local stage_dir=$1
    mkdir -p "${stage_dir}/licenses"
    cp -R licenses/diagnostics/. "${stage_dir}/licenses/"
    verify_diagnostics_notices "${stage_dir}"
}

verify_diagnostics_notices() {
    local stage_dir=$1
    local required
    for required in "${DIAGNOSTICS_NOTICES[@]}"; do
        [[ -s ${stage_dir}/licenses/${required} ]] || {
            echo "Missing license notice: ${required}" >&2
            return 1
        }
    done
}

# Debian package that owns a linked library. Used by the dynamic Linux bundle.
linux_package_for_file() {
    local origin=$1
    local owner
    owner=$(dpkg-query -S "${origin}" 2>/dev/null) || owner=
    if [[ -z ${owner} ]]; then
        origin=$(readlink -f "${origin}")
        owner=$(dpkg-query -S "${origin}" 2>/dev/null) || owner=
    fi
    [[ -n ${owner} ]] || return 1
    owner=${owner%%$'\n'*}
    printf '%s\n' "${owner%%: *}"
}
