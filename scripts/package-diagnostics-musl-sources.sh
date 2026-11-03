#!/usr/bin/env bash
set -euo pipefail

MODE=${1:?usage: package-diagnostics-musl-sources.sh package|verify TARGET VERSION}
TARGET=${2:?target is required}
VERSION=${3:?version is required}
DIST=${DIAGNOSTICS_DIST:-dist/diagnostics}
WORK=${DIAGNOSTICS_WORK:-/build/diagnostics-package}
PROJECT_SOURCE=${PROJECT_SOURCE:-/app}
FFMPEG_SOURCE=${FFMPEG_SOURCE:-/src/ffmpeg}
APORTS_GIT=${APORTS_GIT:-/aports}
CUSTOM_ORIGINS=${CUSTOM_ORIGINS:-/build/static-origins.tsv}
BINARY_STAGE=live777-diagnostics-${VERSION}-${TARGET}
BINARY_ARCHIVE=${DIST}/${BINARY_STAGE}.tar.gz
SOURCE_STAGE=live777-diagnostics-sources-${VERSION}-${TARGET}
SOURCE_DIR=${WORK}/sources/${SOURCE_STAGE}
SOURCE_ARCHIVE=${DIST}/${SOURCE_STAGE}.tar.gz
PROVENANCE=${WORK}/${BINARY_STAGE}/third-party/static-libraries.tsv
SOURCE_COUNT=0

sha256_file() {
    sha256sum "$1" | awk '{print $1}'
}

add_source() {
    local kind=$1
    local package=$2
    local origin=$3
    local source_version=$4
    local revision=$5
    local path=$6
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
        "${kind}" "${package}" "${origin}" "${source_version}" "${revision}" "${path}" >> "${SOURCE_DIR}/SOURCES.tsv"
    SOURCE_COUNT=$((SOURCE_COUNT + 1))
}

collect_project_source() {
    local destination=${SOURCE_DIR}/project
    mkdir -p "${destination}"
    [[ -s ${PROJECT_SOURCE}/Cargo.toml && -s ${PROJECT_SOURCE}/scripts/package-diagnostics-musl.sh && \
        -s ${PROJECT_SOURCE}/scripts/package-diagnostics-musl-sources.sh && \
        -s ${PROJECT_SOURCE}/docker/Dockerfile.diagnostics-musl ]] || {
        echo "Live777 project source is incomplete: ${PROJECT_SOURCE}" >&2
        return 1
    }
    tar -czf "${destination}/live777-workspace.tar.gz" \
        --exclude='./.git' --exclude='./target' --exclude='./dist' \
        --exclude='./node_modules' --exclude='./.playwright' \
        -C "${PROJECT_SOURCE}" .
    printf 'origin\tLive777 diagnostics Docker build context\nversion\t%s\n' "${VERSION}" \
        > "${destination}/revision.tsv"
    add_source project live777 live777 "${VERSION}" workspace project/live777-workspace.tar.gz
}

collect_ffmpeg_source() {
    local destination=${SOURCE_DIR}/ffmpeg/ffmpeg-src
    local commit tag
    [[ -x ${FFMPEG_SOURCE}/configure && -s ${FFMPEG_SOURCE}/ffbuild/config.mak ]] || {
        echo "Configured FFmpeg working tree is missing: ${FFMPEG_SOURCE}" >&2
        return 1
    }
    commit=$(git -C "${FFMPEG_SOURCE}" rev-parse HEAD)
    tag=$(git -C "${FFMPEG_SOURCE}" describe --tags --exact-match 2>/dev/null || printf 'n8.1.2')
    make -s -C "${FFMPEG_SOURCE}" clean
    mkdir -p "${destination}"
    (cd "${FFMPEG_SOURCE}" && tar --exclude='./.git' -cf - .) | tar -xf - -C "${destination}"
    git -C "${FFMPEG_SOURCE}" diff --binary HEAD > "${SOURCE_DIR}/ffmpeg/tracked-changes.patch" || true
    printf 'origin\thttps://github.com/FFmpeg/FFmpeg\ncommit\t%s\ntag\t%s\n' "${commit}" "${tag}" \
        > "${SOURCE_DIR}/ffmpeg/revision.tsv"
    grep '^FFMPEG_CONFIGURATION=' "${FFMPEG_SOURCE}/ffbuild/config.mak" \
        > "${SOURCE_DIR}/ffmpeg/configuration.txt"
    for option in disable-shared enable-static enable-pic enable-libvpx enable-libx264 enable-libx265 \
        enable-libsvtav1 enable-libdav1d enable-libopus enable-openssl enable-protocol=dtls enable-muxer=whip; do
        grep -q -- "--${option}" "${SOURCE_DIR}/ffmpeg/configuration.txt" || {
            echo "FFmpeg configuration is missing --${option}" >&2
            return 1
        }
    done
    add_source ffmpeg ffmpeg ffmpeg 8.1.2 "${commit}" ffmpeg/ffmpeg-src
}

apk_package_for_owner() {
    local owner=$1
    awk -v owner="${owner}" 'BEGIN { RS=""; FS="\n" }
        {
            package=""; version=""
            for (i = 1; i <= NF; i++) {
                if ($i ~ /^P:/) package=substr($i, 3)
                if ($i ~ /^V:/) version=substr($i, 3)
            }
            if (package != "" && package "-" version == owner) {
                print package
                exit
            }
        }' /lib/apk/db/installed
}

apk_record_field() {
    local package=$1
    local field=$2
    awk -v wanted="${package}" -v field="${field}" 'BEGIN { RS=""; FS="\n" }
        {
            found=0
            for (i = 1; i <= NF; i++) if ($i == "P:" wanted) found=1
            if (found) {
                for (i = 1; i <= NF; i++) {
                    if (index($i, field ":") == 1) {
                        print substr($i, length(field) + 2)
                        exit
                    }
                }
            }
        }' /lib/apk/db/installed
}

custom_origin_for_archive() {
    local archive=$1
    awk -F '\t' -v archive="${archive}" '$1 == archive {print; exit}' "${CUSTOM_ORIGINS}"
}

ensure_aports_revision() {
    local revision=$1
    if ! git -C "${APORTS_GIT}" cat-file -e "${revision}^{commit}" 2>/dev/null; then
        git -C "${APORTS_GIT}" fetch --depth 1 origin "${revision}" || \
            git -C "${APORTS_GIT}" fetch --depth 1 \
                https://gitlab.alpinelinux.org/alpine/aports.git "${revision}"
    fi
}

aports_recipe_path() {
    local revision=$1
    local origin=$2
    local repository
    for repository in main community testing; do
        if git -C "${APORTS_GIT}" cat-file -e "${revision}:${repository}/${origin}/APKBUILD" 2>/dev/null; then
            printf '%s\n' "${repository}/${origin}"
            return 0
        fi
    done
    return 1
}

fetch_aports_distfiles() {
    local recipe_dir=$1
    local origin=$2
    local source_version=$3
    (
        cd "${recipe_dir}"
        export CARCH="$(abuild -A)"
        set +u
        . ./APKBUILD
        for source_uri in ${source}; do
            case ${source_uri#*::} in
                *://*)
                    url=${source_uri#*::}
                    filename=${source_uri%%::*}
                    filename=${filename##*/}
                    if [[ ! -s /var/cache/distfiles/${filename} ]]; then
                        case ${origin} in
                            gcc)
                                url="https://ftp.tsukuba.wide.ad.jp/software/gcc/releases/gcc-${source_version%-r*}/${filename}"
                                ;;
                            musl)
                                url="https://deb.debian.org/debian/pool/main/m/musl/musl_${source_version%-r*}.orig.tar.gz"
                                ;;
                        esac
                        curl -fsSL --retry 8 --retry-all-errors --retry-delay 2 \
                            --connect-timeout 20 --max-time 900 \
                            -o "/var/cache/distfiles/${filename}.part" "${url}"
                        mv "/var/cache/distfiles/${filename}.part" "/var/cache/distfiles/${filename}"
                    fi
                    ;;
            esac
        done
    )
}

collect_aports_origin() {
    local package=$1
    local origin=$2
    local source_version=$3
    local revision=$4
    local recipe destination distfiles
    ensure_aports_revision "${revision}"
    recipe=$(aports_recipe_path "${revision}" "${origin}") || {
        echo "No aports recipe for ${origin} at ${revision}" >&2
        return 1
    }
    destination=${SOURCE_DIR}/aports/${origin}/${source_version}/${revision}
    mkdir -p "${destination}"
    git -C "${APORTS_GIT}" archive "${revision}" "${recipe}" | tar -x -C "${destination}"
    distfiles=${destination}/distfiles
    mkdir -p "${distfiles}"
    fetch_aports_distfiles "${destination}/${recipe}" "${origin}" "${source_version}"
    (if [[ ${origin} == gcc ]]; then \
        export DISTFILES_MIRROR="https://ftp.tsukuba.wide.ad.jp/software/gcc/releases/gcc-${source_version%-r*}"; \
    fi; \
    cd "${destination}/${recipe}" && abuild -F -d fetch verify && \
        export CARCH="$(abuild -A)" && set +u && . ./APKBUILD && \
        for source_uri in ${source}; do \
            case ${source_uri#*::} in \
                *://*) \
                    filename=${source_uri%%::*}; filename=${filename##*/}; \
                    cp "/var/cache/distfiles/${filename}" "${distfiles}/${filename}" \
                    ;; \
            esac; \
        done)
    [[ -s ${destination}/${recipe}/APKBUILD ]] || { echo "Empty APKBUILD for ${origin}" >&2; return 1; }
    [[ -n $(find "${distfiles}" -type f -size +0c -print -quit) ]] || {
        echo "abuild fetched no upstream source payload for ${origin}" >&2
        return 1
    }
    add_source aports-source "${package}" "${origin}" "${source_version}" "${revision}" \
        "aports/${origin}/${source_version}/${revision}"
}

collect_static_dependencies() {
    local archive owner package metadata origin source_version revision key
    local collected=$'\n'
    [[ -s ${PROVENANCE} ]] || { echo "Missing static-link provenance: ${PROVENANCE}" >&2; return 1; }
    [[ -s ${CUSTOM_ORIGINS} ]] || { echo "Missing custom static-library origins: ${CUSTOM_ORIGINS}" >&2; return 1; }
    printf 'package\torigin\tversion\taports_revision\tarchive\n' > "${SOURCE_DIR}/DEPENDENCIES.tsv"
    while IFS= read -r archive; do
        archive=$(readlink -f "${archive}")
        [[ -f ${archive} ]] || { echo "Static link input does not exist: ${archive}" >&2; return 1; }
        case ${archive} in
            /opt/ffmpeg/*) continue ;;
            /opt/static/*)
                metadata=$(custom_origin_for_archive "${archive}")
                [[ -n ${metadata} ]] || { echo "No origin recorded for ${archive}" >&2; return 1; }
                IFS=$'\t' read -r _ package origin source_version revision _ <<< "${metadata}"
                ;;
            /usr/*)
                owner=$(apk info -W "${archive}" 2>/dev/null | awk '/ is owned by / {print $NF; exit}')
                [[ -n ${owner} ]] || { echo "No Alpine package owns ${archive}" >&2; return 1; }
                package=$(apk_package_for_owner "${owner}")
                [[ -n ${package} ]] || { echo "Unable to map Alpine package ${owner}" >&2; return 1; }
                origin=$(apk_record_field "${package}" o)
                source_version=$(apk_record_field "${package}" V)
                revision=$(apk_record_field "${package}" c)
                [[ -n ${origin} && -n ${source_version} && -n ${revision} ]] || {
                    echo "Incomplete Alpine origin metadata for ${package}" >&2
                    return 1
                }
                ;;
            *) echo "Unsupported static link input (not FFmpeg, /opt/static or Alpine): ${archive}" >&2; return 1 ;;
        esac
        printf '%s\t%s\t%s\t%s\t%s\n' "${package}" "${origin}" "${source_version}" "${revision}" "${archive}" \
            >> "${SOURCE_DIR}/DEPENDENCIES.tsv"
        key=${origin}=${source_version}=${revision}
        if [[ ${collected} != *$'\n'"${key}"$'\n'* ]]; then
            collect_aports_origin "${package}" "${origin}" "${source_version}" "${revision}"
            collected+="${key}"$'\n'
        fi
    done < "${PROVENANCE}"
    [[ $(wc -l < "${SOURCE_DIR}/DEPENDENCIES.tsv") -gt 1 ]] || {
        echo "No Alpine static dependencies were collected" >&2
        return 1
    }
    for required in musl gcc libunwind xz openssl libvpx x264 x265 svt-av1 dav1d opus; do
        awk -F '\t' -v origin="${required}" '$2 == origin {found=1} END {exit !found}' "${SOURCE_DIR}/DEPENDENCIES.tsv" || {
            echo "Required static dependency is absent from the final link: ${required}" >&2
            return 1
        }
    done
}

write_rebuild_instructions() {
    cat > "${SOURCE_DIR}/REBUILD.md" <<'REBUILD'
# Rebuilding the musl diagnostics bundle

`MANIFEST.tsv` binds this archive to one binary archive by target, version and
SHA-256. `MANIFEST.sha256` covers every payload file. `DEPENDENCIES.tsv` is
derived from the final GNU ld `LOAD` records, not from a handwritten dependency
list; it maps each static archive to its Alpine binary package, source origin,
version and exact aports commit.

The `project/` archive is the exact Live777 Docker build context, including
both musl packaging scripts and `docker/Dockerfile.diagnostics-musl` with the
custom codec configure/CMake/Meson arguments and the final Rust static-link
definitions. The `ffmpeg/ffmpeg-src/` tree is the configured FFmpeg n8.1.2 working tree after
removing object files; `ffmpeg/configuration.txt` and `ffbuild/config.mak`
retain the complete configuration. Use `make` in that tree to reproduce the
same libraries.

Each `aports/` directory contains the complete recipe at the recorded commit
(including Alpine patches) and a `distfiles/` directory populated and verified
with `abuild -F -d fetch verify`. These are the actual upstream source archives
for the static libraries present in the final link, including musl and the GCC
runtime when used. Custom codec libraries whose Alpine binary packages omit
`.a` files were built from these same patched sources into `/opt/static`.
REBUILD
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
        printf 'dependencies\tDEPENDENCIES.tsv\n'
        printf 'rebuild\tREBUILD.md\n'
    } > "${SOURCE_DIR}/MANIFEST.tsv"
    (cd "${SOURCE_DIR}" && find . -type f ! -name MANIFEST.sha256 -print | LC_ALL=C sort | xargs sha256sum) \
        > "${SOURCE_DIR}/MANIFEST.sha256"
}

package_sources() {
    rm -rf "${SOURCE_DIR}"
    mkdir -p "${SOURCE_DIR}" "${DIST}"
    printf 'kind\tpackage\torigin\tversion\taports_revision\tpath\n' > "${SOURCE_DIR}/SOURCES.tsv"
    cp -R licenses/diagnostics "${SOURCE_DIR}/licenses"
    cp "${PROVENANCE}" "${SOURCE_DIR}/static-libraries.tsv"
    collect_project_source
    collect_ffmpeg_source
    collect_static_dependencies
    write_rebuild_instructions
    write_manifest
    rm -f "${SOURCE_ARCHIVE}"
    tar -czf "${SOURCE_ARCHIVE}" -C "${SOURCE_DIR%/*}" "${SOURCE_STAGE}"
    [[ -s ${SOURCE_ARCHIVE} ]] || { echo "Source archive is empty" >&2; return 1; }
    printf 'source_archive=%s\n' "${SOURCE_ARCHIVE}"
    printf 'source_sha256=%s\n' "$(sha256_file "${SOURCE_ARCHIVE}")"
}

manifest_value() {
    awk -F '\t' -v key="$2" '$1 == key {sub(/^[^\t]*\t/, ""); print; exit}' "$1"
}

verify_sources() {
    local temporary bundle expected actual count kind package origin source_version revision path
    [[ -s ${SOURCE_ARCHIVE} ]] || { echo "Missing source archive: ${SOURCE_ARCHIVE}" >&2; return 1; }
    [[ -s ${BINARY_ARCHIVE} ]] || { echo "Missing binary archive: ${BINARY_ARCHIVE}" >&2; return 1; }
    temporary=$(mktemp -d)
    trap 'rm -rf "${temporary}"' RETURN
    tar -xzf "${SOURCE_ARCHIVE}" -C "${temporary}"
    bundle=${temporary}/${SOURCE_STAGE}
    for path in MANIFEST.tsv MANIFEST.sha256 SOURCES.tsv DEPENDENCIES.tsv REBUILD.md static-libraries.tsv; do
        [[ -s ${bundle}/${path} ]] || { echo "Source archive is missing ${path}" >&2; return 1; }
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
    while IFS=$'\t' read -r kind package origin source_version revision path; do
        [[ ${kind} == kind ]] && continue
        [[ -n ${package} && -n ${origin} && -n ${source_version} && -n ${revision} && -n ${path} ]] || {
            echo "Incomplete source provenance row" >&2
            return 1
        }
        [[ ${path} != /* && ${path} != *..* ]] || { echo "Unsafe source path: ${path}" >&2; return 1; }
        [[ -e ${bundle}/${path} ]] || { echo "Manifest source is missing: ${path}" >&2; return 1; }
        if [[ -f ${bundle}/${path} ]]; then
            [[ -s ${bundle}/${path} ]] || { echo "Manifest source is empty: ${path}" >&2; return 1; }
        else
            [[ -n $(find "${bundle}/${path}" -type f -size +0c -print -quit) ]] || {
                echo "Manifest source directory is empty: ${path}" >&2
                return 1
            }
        fi
        if [[ ${kind} == aports-source ]]; then
            [[ -n $(find "${bundle}/${path}" -name APKBUILD -type f -size +0c -print -quit) ]] || {
                echo "aports recipe is missing: ${path}" >&2
                return 1
            }
            [[ -n $(find "${bundle}/${path}/distfiles" -type f -size +0c -print -quit) ]] || {
                echo "aports upstream source payload is missing: ${path}" >&2
                return 1
            }
        fi
    done < "${bundle}/SOURCES.tsv"
    [[ -f ${bundle}/ffmpeg/ffmpeg-src/configure && -s ${bundle}/ffmpeg/configuration.txt ]] || {
        echo "Exact FFmpeg source or configuration is missing" >&2
        return 1
    }
    tar -tzf "${bundle}/project/live777-workspace.tar.gz" | grep '^\./Cargo\.toml$' >/dev/null || {
        echo "Live777 project source is missing Cargo.toml" >&2
        return 1
    }
    tar -tzf "${bundle}/project/live777-workspace.tar.gz" | \
        grep '^\./docker/Dockerfile\.diagnostics-musl$' >/dev/null || {
        echo "Live777 project source is missing the musl build recipe" >&2
        return 1
    }
    (cd "${bundle}" && sha256sum -c MANIFEST.sha256)
}

case ${MODE} in
    package) package_sources ;;
    verify) verify_sources ;;
    *) echo "Unknown mode: ${MODE}" >&2; exit 2 ;;
esac
