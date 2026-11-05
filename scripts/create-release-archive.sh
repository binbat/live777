#!/usr/bin/env bash
set -Eeuo pipefail

APP=${1:?usage: create-release-archive.sh APP TARGET VERSION BUILD_PATH EXE_SUFFIX [VARIANT]}
TARGET=${2:?target is required}
VERSION=${3:?version is required}
BUILD_PATH=${4:?build path is required}
EXE_SUFFIX=${5:-}
VARIANT=${6:-}
TAR_BASENAME="${APP}-${VERSION}-${TARGET}${VARIANT:+-${VARIANT}}"
TAR_SUFFIX=.tar.gz
case "${TARGET}" in
    *-pc-windows-*) TAR_SUFFIX=.zip ;;
esac
TAR_NAME="${TAR_BASENAME}${TAR_SUFFIX}"
ARCHIVE_DIR="${TAR_BASENAME}/"

mkdir -p "${ARCHIVE_DIR}"
cp "${BUILD_PATH}/${APP}${EXE_SUFFIX}" "${ARCHIVE_DIR}"
cp README.md LICENSE "conf/${APP}.toml" "conf/${APP}.service" "${ARCHIVE_DIR}"

case "${TARGET}" in
    *-pc-windows-*) 7z -y a "${TAR_NAME}" "${TAR_BASENAME}"/* | tail -2 ;;
    *) tar czf "${TAR_NAME}" "${TAR_BASENAME}"/* ;;
esac

if [[ -n ${GITHUB_ENV:-} ]]; then
    ENV_NAME=$(printf '%s' "${APP}" | tr '[:lower:]' '[:upper:]')
    printf 'TAR_%s_NAME=%s\n' "${ENV_NAME}" "${TAR_NAME}" >> "${GITHUB_ENV}"
fi
printf '%s\n' "${TAR_NAME}"
