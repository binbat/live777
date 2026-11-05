#!/usr/bin/env bash
set -euo pipefail

MODE=${1:?usage: package-diagnostics-sources.sh package|verify TARGET VERSION}
TARGET=${2:?target is required}
VERSION=${3:?version is required}
WORKSPACE=${GITHUB_WORKSPACE:-$PWD}
DIST=dist/diagnostics
BINARY_STAGE=live777-diagnostics-${VERSION}-${TARGET}
BINARY_ARCHIVE=${DIST}/${BINARY_STAGE}.tar.gz
SOURCE_STAGE=live777-diagnostics-sources-${VERSION}-${TARGET}
SOURCE_DIR=${DIST}/sources-work/${SOURCE_STAGE}
SOURCE_ARCHIVE=${DIST}/${SOURCE_STAGE}.tar.gz
PROVENANCE=${DIST}/work/${BINARY_STAGE}/third-party/copied-files.tsv
SOURCE_COUNT=0

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=lib/diagnostics.sh
source "${SCRIPT_DIR}/lib/diagnostics.sh"

add_source() {
    local kind=$1
    local name=$2
    local source_version=$3
    local path=$4
    printf '%s\t%s\t%s\t%s\n' "${kind}" "${name}" "${source_version}" "${path}" >> "${SOURCE_DIR}/SOURCES.tsv"
    SOURCE_COUNT=$((SOURCE_COUNT + 1))
}

collect_project_source() {
    local destination=${SOURCE_DIR}/project
    local commit
    mkdir -p "${destination}"
    commit=$(git rev-parse HEAD)
    git archive --format=zip --output="${destination}/live777-${commit}.zip" HEAD
    git diff --binary HEAD > "${destination}/tracked-changes.patch"
    git status --porcelain > "${destination}/workspace-status.txt"
    add_source project live777 "${commit}" "project"
}

collect_ffmpeg_source() {
    local tag source destination commit
    case $(uname -s) in
        Linux) tag=n8.1.2 ;;
        Darwin) tag=n8.1.2 ;;
        *) echo "Unsupported Unix host: $(uname -s)" >&2; return 1 ;;
    esac
    source=${WORKSPACE}/ffmpeg-src
    if [[ ! -f ${source}/configure ]]; then
        source=${DIST}/source-input/ffmpeg-src
        rm -rf "${source}"
        mkdir -p "${source%/*}"
        git clone --depth 1 --branch "${tag}" https://github.com/FFmpeg/FFmpeg "${source}"
    fi
    [[ -f ${source}/configure ]] || { echo "FFmpeg source is missing: ${source}" >&2; return 1; }
    commit=$(git -C "${source}" rev-parse HEAD)
    destination=${SOURCE_DIR}/ffmpeg/ffmpeg-src
    mkdir -p "${destination}"
    (cd "${source}" && tar --exclude='./.git' -cf - .) | tar -xf - -C "${destination}"
    (cd "${source}" && git diff --binary HEAD) > "${SOURCE_DIR}/ffmpeg/tracked-changes.patch"
    printf 'origin\t%s\ncommit\t%s\ntag\t%s\n' "${source}" "${commit}" "${tag}" > "${SOURCE_DIR}/ffmpeg/revision.tsv"
    if [[ -f ${source}/ffbuild/config.mak ]]; then
        grep '^FFMPEG_CONFIGURATION=' "${source}/ffbuild/config.mak" > "${SOURCE_DIR}/ffmpeg/configuration.txt" || true
    fi
    if [[ ! -s ${SOURCE_DIR}/ffmpeg/configuration.txt && -x ${WORKSPACE}/ffmpeg-install/bin/ffmpeg ]]; then
        "${WORKSPACE}/ffmpeg-install/bin/ffmpeg" -version 2>/dev/null |
            awk '/^configuration:/ {sub(/^configuration: /, ""); print; exit}' > "${SOURCE_DIR}/ffmpeg/configuration.txt"
    fi
    [[ -s ${SOURCE_DIR}/ffmpeg/configuration.txt ]] || { echo "Unable to record the exact FFmpeg configuration" >&2; return 1; }
    add_source ffmpeg ffmpeg "${tag}-${commit}" ffmpeg/ffmpeg-src
}

collect_linux_sources() {
    local origin package metadata source_package source_version destination key
    local found=false
    local seen=$'\n'
    [[ -s ${PROVENANCE} ]] || { echo "Missing copied-library provenance: ${PROVENANCE}" >&2; return 1; }
    while IFS=$'\t' read -r _ origin; do
        [[ ${origin} == "${WORKSPACE}/ffmpeg-install/"* ]] && continue
        package=$(linux_package_for_file "${origin}") || {
            echo "No Debian package owns copied library ${origin}" >&2
            return 1
        }
        metadata=$(dpkg-query -W -f='${binary:Package}\t${source:Package}\t${source:Version}\n' "${package}")
        IFS=$'\t' read -r _ source_package source_version <<< "${metadata}"
        [[ -n ${source_package} && -n ${source_version} ]] || {
            echo "Unable to resolve the exact source version for ${package}" >&2
            return 1
        }
        key=${source_package}=${source_version}
        [[ ${seen} == *$'\n'"${key}"$'\n'* ]] && continue
        seen+="${key}"$'\n'
        destination=${SOURCE_DIR}/apt/${source_package}/${source_version}
        mkdir -p "${destination}"
        (cd "${destination}" && apt-get source --download-only "${key}")
        compgen -G "${destination}/*.dsc" >/dev/null || {
            echo "apt-get source produced no .dsc for ${key}" >&2
            return 1
        }
        [[ -n $(find "${destination}" -maxdepth 1 -type f ! -name '*.dsc' -print -quit) ]] || {
            echo "apt-get source produced no source payload for ${key}" >&2
            return 1
        }
        add_source apt-source "${source_package}" "${source_version}" "apt/${source_package}/${source_version}"
        found=true
    done < "${PROVENANCE}"
    [[ ${found} == true ]] || { echo "No Linux dependency source packages collected" >&2; return 1; }
}

homebrew_formula_for_file() {
    local resolved cellar relative
    resolved=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$1")
    cellar=$(brew --cellar)
    [[ ${resolved} == "${cellar}/"* ]] || return 1
    relative=${resolved#"${cellar}"/}
    printf '%s\n' "${relative%%/*}"
}

collect_macos_sources() {
    local origin formula prefix cache destination expected actual found=false
    local seen=$'\n'
    [[ -s ${PROVENANCE} ]] || { echo "Missing copied-library provenance: ${PROVENANCE}" >&2; return 1; }
    mkdir -p "${SOURCE_DIR}/homebrew/downloads" "${SOURCE_DIR}/homebrew/formula"
    while IFS=$'\t' read -r _ origin; do
        [[ ${origin} == "${WORKSPACE}/ffmpeg-install/"* ]] && continue
        formula=$(homebrew_formula_for_file "${origin}") || {
            echo "Copied library is not from Homebrew: ${origin}" >&2
            return 1
        }
        [[ ${seen} == *$'\n'"${formula}"$'\n'* ]] && continue
        seen+="${formula}"$'\n'
        prefix=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$(brew --prefix "${formula}")")
        cache=$(brew --cache --formula --build-from-source "${formula}")
        if [[ ! -d ${cache} && ! -s ${cache} ]]; then
            brew fetch --formula --build-from-source "${formula}"
            cache=$(brew --cache --formula --build-from-source "${formula}")
        fi
        [[ -d ${cache} || -s ${cache} ]] || { echo "Homebrew source cache is missing for ${formula}: ${cache}" >&2; return 1; }
        destination=${SOURCE_DIR}/homebrew/formula/${formula}
        mkdir -p "${destination}"
        brew cat "${formula}" > "${destination}/${formula}.rb"
        brew info --json=v2 --formula "${formula}" > "${destination}/info.json"
        [[ -f ${prefix}/INSTALL_RECEIPT.json ]] || { echo "Missing Homebrew install receipt for ${formula}" >&2; return 1; }
        cp "${prefix}/INSTALL_RECEIPT.json" "${destination}/INSTALL_RECEIPT.json"
        expected=$(python3 - "${destination}/info.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    formula = json.load(handle)["formulae"][0]
version = formula["versions"]["stable"]
revision = formula.get("revision", 0)
print(version if not revision else f"{version}_{revision}")
PY
)
        actual=${prefix##*/}
        [[ ${actual} == "${expected}" ]] || {
            echo "Homebrew formula ${formula} provides ${expected}, but copied libraries came from ${actual}" >&2
            return 1
        }
        cp -R "${cache}" "${SOURCE_DIR}/homebrew/downloads/${cache##*/}"
        add_source homebrew-formula "${formula}" "${actual}" "homebrew/formula/${formula}"
        add_source homebrew-source "${formula}" "${actual}" "homebrew/downloads/${cache##*/}"
        found=true
    done < "${PROVENANCE}"
    [[ ${found} == true ]] || { echo "No Homebrew source archives collected" >&2; return 1; }
}

write_rebuild_instructions() {
    cat > "${SOURCE_DIR}/REBUILD.md" <<'REBUILD'
# Rebuilding the corresponding diagnostics sources

`MANIFEST.tsv` binds this archive to one binary diagnostics archive by target,
version and SHA-256. `SOURCES.tsv` lists the source payloads. The `project/`
zip is the Live777 source at the recorded Git commit; apply
`tracked-changes.patch` if it is non-empty. Build the three programs with:

```sh
cargo build --locked --release --target=<target> \
  --bin=whipsynth --bin=whepprobe --bin=livewrk --features=rsmpeg
```

The exact upstream FFmpeg working tree (including generated configuration but
excluding `.git`) is in `ffmpeg/ffmpeg-src/`. `ffmpeg/configuration.txt` records
the configuration used for the distributed libraries. Apply
`ffmpeg/tracked-changes.patch` if non-empty, configure with those arguments,
then run `make` and `make install`. Use a writable prefix rather than the
original CI prefix.
REBUILD
    case $(uname -s) in
        Linux)
            cat >> "${SOURCE_DIR}/REBUILD.md" <<'REBUILD'

## Debian/Ubuntu dependency sources

Each `apt/<source>/<version>/` directory contains the exact `.dsc` and source
archives downloaded with `apt-get source --download-only <source>=<version>`.
Unpack one with `dpkg-source -x <file>.dsc` and follow its `debian/README.source`
and `debian/rules`. These are source packages for the libraries actually copied
into the binary bundle, including codec, TLS and compiler-runtime packages when
present. The release workflow enables `deb-src` before collection; no missing
source package is skipped.
REBUILD
            ;;
        Darwin)
            cat >> "${SOURCE_DIR}/REBUILD.md" <<'REBUILD'

## Homebrew dependency sources

`homebrew/formula/<name>/` contains the exact formula, JSON version metadata
and install receipt. `homebrew/downloads/` contains the matching source cache,
not a bottle. Restore those files to Homebrew's `downloads` cache, install the
included formulas with `brew install --build-from-source`, and use the formula
metadata to select the recorded version and revision. The source collection
fails if the currently installed Cellar version differs from the formula.
REBUILD
            ;;
    esac
}

write_manifest() {
    local binary_hash
    [[ -s ${BINARY_ARCHIVE} ]] || { echo "Missing binary diagnostics archive: ${BINARY_ARCHIVE}" >&2; return 1; }
    binary_hash=$(sha256_file "${BINARY_ARCHIVE}")
    [[ ${SOURCE_COUNT} -gt 0 ]] || { echo "Refusing to create an empty source package" >&2; return 1; }
    {
        printf 'format\t1\n'
        printf 'target\t%s\n' "${TARGET}"
        printf 'version\t%s\n' "${VERSION}"
        printf 'binary_archive\t%s\n' "${BINARY_ARCHIVE##*/}"
        printf 'binary_sha256\t%s\n' "${binary_hash}"
        printf 'source_count\t%s\n' "${SOURCE_COUNT}"
        printf 'sources\tSOURCES.tsv\n'
        printf 'rebuild\tREBUILD.md\n'
    } > "${SOURCE_DIR}/MANIFEST.tsv"
}

package_sources() {
    rm -rf "${SOURCE_DIR}"
    mkdir -p "${SOURCE_DIR}" "${DIST}"
    printf 'kind\tname\tversion\tpath\n' > "${SOURCE_DIR}/SOURCES.tsv"
    cp -R licenses/diagnostics "${SOURCE_DIR}/licenses"
    [[ -s ${PROVENANCE} ]] || { echo "Missing copied-library provenance: ${PROVENANCE}" >&2; return 1; }
    mkdir -p "${SOURCE_DIR}/third-party"
    cp "${PROVENANCE}" "${SOURCE_DIR}/third-party/copied-files.tsv"
    collect_project_source
    collect_ffmpeg_source
    case $(uname -s) in
        Linux) collect_linux_sources ;;
        Darwin) collect_macos_sources ;;
        *) echo "Unsupported Unix host: $(uname -s)" >&2; return 1 ;;
    esac
    write_rebuild_instructions
    write_manifest
    rm -f "${SOURCE_ARCHIVE}"
    tar -czf "${SOURCE_ARCHIVE}" -C "${SOURCE_DIR%/*}" "${SOURCE_STAGE}"
    [[ -s ${SOURCE_ARCHIVE} ]] || { echo "Source archive is empty" >&2; return 1; }
    if [[ -n ${GITHUB_OUTPUT:-} ]]; then
        echo "source_archive=${SOURCE_ARCHIVE}" >> "${GITHUB_OUTPUT}"
    fi
}

verify_sources() {
    local temporary bundle expected actual required count kind name source_version path
    [[ -s ${SOURCE_ARCHIVE} ]] || { echo "Missing source archive: ${SOURCE_ARCHIVE}" >&2; return 1; }
    [[ -s ${BINARY_ARCHIVE} ]] || { echo "Missing binary archive: ${BINARY_ARCHIVE}" >&2; return 1; }
    temporary=$(mktemp -d)
    trap 'rm -rf "${temporary}"' RETURN
    tar -xzf "${SOURCE_ARCHIVE}" -C "${temporary}"
    bundle=${temporary}/${SOURCE_STAGE}
    for required in MANIFEST.tsv SOURCES.tsv REBUILD.md third-party/copied-files.tsv; do
        [[ -s ${bundle}/${required} ]] || { echo "Source archive is missing ${required}" >&2; return 1; }
    done
    [[ $(manifest_value "${bundle}/MANIFEST.tsv" target) == "${TARGET}" ]] || { echo "Source target mismatch" >&2; return 1; }
    [[ $(manifest_value "${bundle}/MANIFEST.tsv" version) == "${VERSION}" ]] || { echo "Source version mismatch" >&2; return 1; }
    [[ $(manifest_value "${bundle}/MANIFEST.tsv" binary_archive) == "${BINARY_ARCHIVE##*/}" ]] || {
        echo "Source binary archive name mismatch" >&2
        return 1
    }
    expected=$(manifest_value "${bundle}/MANIFEST.tsv" binary_sha256)
    actual=$(sha256_file "${BINARY_ARCHIVE}")
    [[ ${expected} == "${actual}" ]] || { echo "Binary SHA-256 does not match source manifest" >&2; return 1; }
    count=$(manifest_value "${bundle}/MANIFEST.tsv" source_count)
    [[ ${count} =~ ^[1-9][0-9]*$ ]] || { echo "Source manifest has no sources" >&2; return 1; }
    while IFS=$'\t' read -r kind name source_version path; do
        [[ ${kind} == kind ]] && continue
        [[ -n ${path} && ${path} != /* && ${path} != *..* ]] || { echo "Unsafe source path: ${path}" >&2; return 1; }
        [[ -e ${bundle}/${path} ]] || { echo "Manifest source is missing: ${path}" >&2; return 1; }
        if [[ -f ${bundle}/${path} ]]; then
            [[ -s ${bundle}/${path} ]] || { echo "Manifest source is empty: ${path}" >&2; return 1; }
        else
            [[ -n $(find "${bundle}/${path}" -type f -print -quit) ]] || {
                echo "Manifest source directory is empty: ${path}" >&2
                return 1
            }
        fi
        if [[ ${kind} == apt-source ]]; then
            compgen -G "${bundle}/${path}/*.dsc" >/dev/null || { echo "apt source .dsc is missing: ${path}" >&2; return 1; }
            [[ -n $(find "${bundle}/${path}" -maxdepth 1 -type f ! -name '*.dsc' -print -quit) ]] || {
                echo "apt source payload is missing: ${path}" >&2
                return 1
            }
        fi
    done < "${bundle}/SOURCES.tsv"
    [[ -f ${bundle}/ffmpeg/ffmpeg-src/configure && -s ${bundle}/ffmpeg/configuration.txt ]] || {
        echo "Exact FFmpeg source or configuration is missing" >&2
        return 1
    }
    compgen -G "${bundle}/project/live777-*.zip" >/dev/null || { echo "Live777 project source is missing" >&2; return 1; }
}

case ${MODE} in
    package) package_sources ;;
    verify) verify_sources ;;
    *) echo "Unknown mode: ${MODE}" >&2; exit 2 ;;
esac
