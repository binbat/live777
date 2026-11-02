#!/usr/bin/env bash
set -Eeuo pipefail

registry_name="live777-registry-test-$$"
builder_name="live777-registry-test-$$"
repository="live777-registry-test"
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/live777-registry-test.XXXXXX")
local_images=()
registry_started=false
builder_created=false

cleanup() {
    status=$?
    if [[ ${builder_created} == true ]]; then
        docker buildx rm --force "${builder_name}" >/dev/null 2>&1 || true
    fi
    if [[ ${registry_started} == true ]]; then
        docker rm --force "${registry_name}" >/dev/null 2>&1 || true
    fi
    for image in "${local_images[@]}"; do
        docker image rm --force "${image}" >/dev/null 2>&1 || true
    done
    rm -rf "${tmp_dir}"
    exit "${status}"
}
trap cleanup EXIT

for command in curl docker jq; do
    command -v "${command}" >/dev/null || {
        echo "Required command not found: ${command}" >&2
        exit 1
    }
done
docker buildx version >/dev/null

registry_started=true
docker run --rm -d --name "${registry_name}" --publish 127.0.0.1::5000 registry:2 >/dev/null
mapped_port=$(docker port "${registry_name}" 5000/tcp)
registry="127.0.0.1:${mapped_port##*:}"
image="${registry}/${repository}"
registry_url="http://${registry}"
for ((i = 0; i < 30; i++)); do
    if curl -fsS --max-time 2 "${registry_url}/v2/" >/dev/null; then
        break
    fi
    if ((i == 29)); then
        echo "Local registry did not become ready" >&2
        exit 1
    fi
    sleep 1
done

builder_created=true
docker buildx create \
    --name "${builder_name}" \
    --driver docker-container \
    --driver-opt network=host \
    --bootstrap >/dev/null

cat >"${tmp_dir}/Dockerfile" <<'EOF'
FROM busybox:1.37.0
CMD ["/bin/echo", "registry-integration-ok"]
EOF

declare -A digests=()
for arch in amd64 arm64; do
    archive="${tmp_dir}/image-${arch}.tar"
    metadata="${tmp_dir}/metadata-${arch}.json"
    local_image="local/docker-registry-test-${arch}:$$"
    local_images+=("${local_image}")

    docker buildx build \
        --builder "${builder_name}" \
        --platform "linux/${arch}" \
        --output "type=docker,dest=${archive},name=${local_image}" \
        --output "type=image,name=${image},push=true,push-by-digest=true" \
        --metadata-file "${metadata}" \
        --file "${tmp_dir}/Dockerfile" \
        "${tmp_dir}"

    digest=$(jq -er '."containerimage.digest"' "${metadata}")
    [[ ${digest} == sha256:* ]] || {
        echo "Invalid ${arch} image digest: ${digest}" >&2
        exit 1
    }
    digests[${arch}]=${digest}
    docker buildx imagetools inspect "${image}@${digest}" >/dev/null

    docker load --input "${archive}" >/dev/null
    docker image inspect "${local_image}" >/dev/null
done

tags_status=$(curl -sS \
    --output "${tmp_dir}/tags.json" \
    --write-out '%{http_code}' \
    "${registry_url}/v2/${repository}/tags/list")
case ${tags_status} in
    200)
        if ! jq -e '(.tags // []) | length == 0' "${tmp_dir}/tags.json" >/dev/null; then
            echo "Digest-only pushes unexpectedly created tags: $(<"${tmp_dir}/tags.json")" >&2
            exit 1
        fi
        ;;
    404) ;;
    *)
        echo "Unable to query local registry tags (HTTP ${tags_status})" >&2
        exit 1
        ;;
esac

docker buildx imagetools create \
    --tag "${image}:test" \
    "${image}@${digests[amd64]}" \
    "${image}@${digests[arm64]}"
docker buildx imagetools inspect --raw "${image}:test" >"${tmp_dir}/manifest.json"
if ! jq -e '
    ([.manifests[] | select(.platform.os == "linux") | .platform.architecture] | sort)
    == ["amd64", "arm64"]
' "${tmp_dir}/manifest.json" >/dev/null; then
    echo "Final manifest does not contain exactly linux/amd64 and linux/arm64" >&2
    cat "${tmp_dir}/manifest.json" >&2
    exit 1
fi

local_images+=("${image}:test")
docker pull "${image}:test"
run_output=$(docker run --rm "${image}:test")
[[ ${run_output} == registry-integration-ok ]] || {
    echo "Unexpected output from pulled image: ${run_output}" >&2
    exit 1
}

printf 'Local registry integration test passed (amd64 %s, arm64 %s)\n' \
    "${digests[amd64]}" "${digests[arm64]}"
