#!/usr/bin/env bash
set -euo pipefail

MODE=${1:?usage: package-diagnostics.sh package|verify TARGET VERSION}
TARGET=${2:?target is required}
VERSION=${3:?version is required}
STAGE=live777-diagnostics-${VERSION}-${TARGET}
DIST=dist/diagnostics
ARCHIVE=${DIST}/${STAGE}.tar.gz
TOOLS=(whipsynth whepprobe livewrk)

record_copied_file() {
    local lib_dir=$1
    local copied=$2
    local origin=$3
    printf 'lib/%s\t%s\n' "${copied}" "${origin}" >> "${lib_dir}/../third-party/copied-files.tsv"
}

copy_common_notices() {
    local stage_dir=$1
    local required
    mkdir -p "${stage_dir}/licenses"
    cp -R licenses/diagnostics/. "${stage_dir}/licenses/"
    for required in NOTICE.md GPL-2.0.txt LGPL-2.1.txt GPL-3.0.txt LGPL-3.0.txt Apache-2.0.txt FFmpeg-LICENSE.md; do
        [[ -s ${stage_dir}/licenses/${required} ]] || { echo "Missing license notice: ${required}" >&2; return 1; }
    done
}

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

copy_linux_notices() {
    local stage_dir=$1
    local copied origin package document destination
    local found=false
    while IFS=$'\t' read -r copied origin; do
        [[ ${origin} == "${GITHUB_WORKSPACE:-$PWD}/ffmpeg-install/"* ]] && continue
        package=$(linux_package_for_file "${origin}") || {
            echo "No Debian package owns copied library ${origin}" >&2
            return 1
        }
        document=/usr/share/doc/${package%%:*}/copyright
        [[ -f ${document} ]] || { echo "Missing copyright notice for ${package}" >&2; return 1; }
        destination=${stage_dir}/licenses/packages/${package//:/_}.copyright
        mkdir -p "${destination%/*}"
        cp -L "${document}" "${destination}"
        found=true
    done < "${stage_dir}/third-party/copied-files.tsv"
    [[ ${found} == true ]] || { echo "No packaged Linux dependency notices found" >&2; return 1; }
}

copy_linux_dependencies() {
    local source=$1
    local lib_dir=$2
    local queue=("$source")
    local current line dependency name resolved
    local seen=$'\n'

    while ((${#queue[@]})); do
        current=${queue[0]}
        queue=("${queue[@]:1}")
        [[ ${seen} == *$'\n'"${current}"$'\n'* ]] && continue
        seen+="${current}"$'\n'

        while IFS= read -r line; do
            [[ ${line} == *"not found"* ]] && { echo "Unresolved dependency: ${line}" >&2; return 1; }
            if [[ ${line} =~ =\>[[:space:]]+(/[^[:space:]]+) ]]; then
                dependency=${BASH_REMATCH[1]}
            elif [[ ${line} =~ ^[[:space:]]*(/[^[:space:]]+) ]]; then
                dependency=${BASH_REMATCH[1]}
            else
                continue
            fi
            name=${dependency##*/}
            case ${name} in
                ld-linux*|libc.so.*|libm.so.*|libmvec.so.*|libdl.so.*|libpthread.so.*|librt.so.*|libresolv.so.*|libutil.so.*|libnsl.so.*)
                    continue
                    ;;
            esac
            [[ -f ${dependency} ]] || { echo "Missing dependency: ${dependency}" >&2; return 1; }
            resolved=${lib_dir}/${name}
            if [[ ! -f ${resolved} ]]; then
                cp -L "${dependency}" "${resolved}"
                chmod u+w,a+x "${resolved}"
                record_copied_file "${lib_dir}" "${name}" "${dependency}"
                queue+=("${resolved}")
            fi
        done < <(LD_LIBRARY_PATH="${lib_dir}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}" ldd "${current}")
    done
}

add_macos_rpath() {
    local file=$1
    local rpath=$2
    if ! otool -l "${file}" | grep -F "path ${rpath} (offset"; then
        install_name_tool -add_rpath "${rpath}" "${file}"
    fi
}

copy_macos_dependencies() {
    local source=$1
    local lib_dir=$2
    local queue=("$source")
    local current dependency name resolved candidate
    local homebrew_lib
    local seen=$'\n'
    homebrew_lib=$(brew --prefix)/lib

    while ((${#queue[@]})); do
        current=${queue[0]}
        queue=("${queue[@]:1}")
        [[ ${seen} == *$'\n'"${current}"$'\n'* ]] && continue
        seen+="${current}"$'\n'

        if [[ $(dirname "${current}") == "${lib_dir}" ]]; then
            install_name_tool -id "@rpath/${current##*/}" "${current}"
            add_macos_rpath "${current}" "@loader_path"
        else
            add_macos_rpath "${current}" "@loader_path/../lib"
        fi

        while IFS= read -r dependency; do
            resolved=
            case ${dependency} in
                /usr/lib/*|/System/*) continue ;;
                @rpath/*|@executable_path/*)
                    name=${dependency#*/}
                    for candidate in "${lib_dir}/${name}" "${GITHUB_WORKSPACE}/ffmpeg-install/lib/${name}" "${homebrew_lib}/${name}"; do
                        if [[ -f ${candidate} ]]; then
                            resolved=${candidate}
                            break
                        fi
                    done
                    ;;
                @loader_path/*)
                    name=${dependency#@loader_path/}
                    resolved=$(dirname "${current}")/${name}
                    ;;
                /*) resolved=${dependency} ;;
                *) echo "Unsupported dependency reference ${dependency} in ${current}" >&2; return 1 ;;
            esac
            [[ -n ${resolved} && -f ${resolved} ]] || { echo "Missing dependency: ${dependency} (${current})" >&2; return 1; }
            name=${resolved##*/}
            if [[ ! -f ${lib_dir}/${name} ]]; then
                cp -L "${resolved}" "${lib_dir}/${name}"
                chmod u+w "${lib_dir}/${name}"
                record_copied_file "${lib_dir}" "${name}" "${resolved}"
                queue+=("${lib_dir}/${name}")
            fi
            if [[ ${dependency} != "@rpath/${name}" ]]; then
                install_name_tool -change "${dependency}" "@rpath/${name}" "${current}"
            fi
        done < <(otool -L "${current}" | tail -n +2 | awk '{print $1}')
    done
}

copy_macos_notices() {
    local stage_dir=$1
    local copied origin resolved relative formula prefix candidate destination
    local cellar found
    local seen=$'\n'
    cellar=$(brew --cellar)
    while IFS=$'\t' read -r copied origin; do
        [[ ${origin} == "${GITHUB_WORKSPACE:-$PWD}/ffmpeg-install/"* ]] && continue
        resolved=$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "${origin}")
        [[ ${resolved} == "${cellar}/"* ]] || { echo "Copied library is not from Homebrew: ${resolved}" >&2; return 1; }
        relative=${resolved#"${cellar}"/}
        formula=${relative%%/*}
        [[ ${seen} == *$'\n'"${formula}"$'\n'* ]] && continue
        seen+="${formula}"$'\n'
        prefix=$(brew --prefix "${formula}")
        found=false
        while IFS= read -r candidate; do
            destination=${stage_dir}/licenses/homebrew/${formula}/${candidate#"${prefix}"/}
            mkdir -p "${destination%/*}"
            cp -L "${candidate}" "${destination}"
            found=true
        done < <(find -L "${prefix}" -type f \( -iname 'COPYING*' -o -iname 'LICENSE*' -o -iname 'NOTICE*' \) -print)
        [[ ${found} == true ]] || { echo "No license notice installed for Homebrew formula ${formula}" >&2; return 1; }
        if [[ -f ${prefix}/INSTALL_RECEIPT.json ]]; then
            cp "${prefix}/INSTALL_RECEIPT.json" "${stage_dir}/licenses/homebrew/${formula}/INSTALL_RECEIPT.json"
        fi
    done < "${stage_dir}/third-party/copied-files.tsv"
}

sign_macos_bundle() {
    local stage_dir=$1
    local tool library
    for tool in "${TOOLS[@]}"; do
        codesign --force --sign - "${stage_dir}/bin/${tool}.real"
    done
    for library in "${stage_dir}/lib/"*.dylib; do
        [[ -e ${library} ]] || continue
        codesign --force --sign - "${library}"
    done
}

verify_macos_bundle() {
    local bundle=$1
    local file dependency
    for file in "${bundle}/bin/"*.real "${bundle}/lib/"*.dylib; do
        [[ -e ${file} ]] || continue
        codesign --verify --strict "${file}"
        otool -l "${file}" | grep -F '@loader_path' || {
            echo "Missing relative rpath in ${file}" >&2
            return 1
        }
        while IFS= read -r dependency; do
            case ${dependency} in
                @rpath/*|/usr/lib/*|/System/*) ;;
                *) echo "Non-relocatable dependency ${dependency} in ${file}" >&2; return 1 ;;
            esac
        done < <(otool -L "${file}" | tail -n +2 | awk '{print $1}')
    done
}

package_unix() {
    local build_dir=${CARGO_TARGET_DIR:-target}/${TARGET}/release
    local work=${DIST}/work
    local stage_dir=${work}/${STAGE}
    local tool

    rm -rf "${stage_dir}"
    mkdir -p "${stage_dir}/bin" "${stage_dir}/lib" "${stage_dir}/third-party" "${DIST}"
    copy_common_notices "${stage_dir}"
    for tool in "${TOOLS[@]}"; do
        cp "${build_dir}/${tool}" "${stage_dir}/bin/${tool}.real"
        cat > "${stage_dir}/bin/${tool}" <<'LAUNCHER'
#!/bin/sh
set -eu
bin_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
if [ "$(uname -s)" != "Darwin" ]; then
    lib_dir="${bin_dir}/../lib"
    LD_LIBRARY_PATH="${lib_dir}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
    export LD_LIBRARY_PATH
fi
tool=${0##*/}
exec "${bin_dir}/${tool}.real" "$@"
LAUNCHER
        chmod +x "${stage_dir}/bin/${tool}"
    done
    cp LICENSE "${stage_dir}/LICENSE"

    case $(uname -s) in
        Linux)
            for tool in "${TOOLS[@]}"; do
                copy_linux_dependencies "${stage_dir}/bin/${tool}.real" "${stage_dir}/lib"
            done
            copy_linux_notices "${stage_dir}"
            ;;
        Darwin)
            for tool in "${TOOLS[@]}"; do
                copy_macos_dependencies "${stage_dir}/bin/${tool}.real" "${stage_dir}/lib"
            done
            copy_macos_notices "${stage_dir}"
            sign_macos_bundle "${stage_dir}"
            ;;
        *) echo "Unsupported Unix host: $(uname -s)" >&2; return 1 ;;
    esac

    rm -f "${ARCHIVE}"
    tar -czf "${ARCHIVE}" -C "${work}" "${STAGE}"
    if [[ -n ${GITHUB_OUTPUT:-} ]]; then
        echo "archive=${ARCHIVE}" >> "${GITHUB_OUTPUT}"
    fi
}

verify_unix() {
    local temporary bundle tool dependencies required
    temporary=$(mktemp -d)
    trap 'rm -rf "${temporary}"' RETURN
    mkdir -p "${temporary}/home"
    tar -xzf "${ARCHIVE}" -C "${temporary}"
    mv "${temporary}/${STAGE}" "${temporary}/relocated bundle"
    bundle="${temporary}/relocated bundle"
    for required in NOTICE.md GPL-2.0.txt LGPL-2.1.txt GPL-3.0.txt LGPL-3.0.txt Apache-2.0.txt FFmpeg-LICENSE.md; do
        [[ -s ${bundle}/licenses/${required} ]] || { echo "Missing bundled license: ${required}" >&2; return 1; }
    done
    [[ -s ${bundle}/third-party/copied-files.tsv ]] || { echo "Missing copied-library provenance" >&2; return 1; }
    case $(uname -s) in
        Linux)
            compgen -G "${bundle}/licenses/packages/*.copyright" >/dev/null || {
                echo "Missing Debian dependency copyright notices" >&2
                return 1
            }
            ;;
        Darwin)
            [[ -n $(find "${bundle}/licenses/homebrew" -type f -print -quit) ]] || {
                echo "Missing Homebrew dependency license notices" >&2
                return 1
            }
            verify_macos_bundle "${bundle}"
            ;;
    esac

    for tool in "${TOOLS[@]}"; do
        env -i PATH=/usr/bin:/bin HOME="${temporary}/home" TMPDIR="${temporary}" \
            "${bundle}/bin/${tool}" --version
        if [[ $(uname -s) == Linux ]]; then
            dependencies=$(env -i PATH=/usr/bin:/bin LD_LIBRARY_PATH="${bundle}/lib" \
                ldd "${bundle}/bin/${tool}.real")
            printf '%s\n' "${dependencies}"
            if grep -q 'not found' <<< "${dependencies}"; then
                echo "Unresolved libraries in ${tool}" >&2
                return 1
            fi
        fi
    done
}

case ${MODE} in
    package) package_unix ;;
    verify) verify_unix ;;
    *) echo "Unknown mode: ${MODE}" >&2; exit 2 ;;
esac
