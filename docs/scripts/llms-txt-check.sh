#!/usr/bin/env bash
set -Eeuo pipefail

BASE_URL="${1:-${DOCS_BASE_URL:-https://live777.pages.dev}}"
BASE_URL="${BASE_URL%/}"
TIMEOUT="${LLMS_CHECK_TIMEOUT:-300}"
INTERVAL="${LLMS_CHECK_INTERVAL:-15}"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

paths=(
    llms.txt
    llms-full.txt
    guide/getting-started.md
    guide/whipinto.md
    guide/whepfrom.md
)

deadline=$((SECONDS + TIMEOUT))
while :; do
    ready=1
    for path in "${paths[@]}"; do
        if ! curl -fsS --max-time 20 "$BASE_URL/$path" -o "$TMP_DIR/${path//\//_}"; then
            ready=0
            break
        fi
    done

    if ((ready == 1)) &&
        grep -q 'guide/getting-started.md' "$TMP_DIR/llms.txt" &&
        grep -q 'guide/whipinto.md' "$TMP_DIR/llms.txt" &&
        grep -q 'guide/whepfrom.md' "$TMP_DIR/llms.txt" &&
        grep -q 'Verify the RTP bridge in three terminals' "$TMP_DIR/guide_getting-started.md" &&
        grep -q '## Troubleshooting' "$TMP_DIR/guide_whipinto.md" &&
        grep -q '## Troubleshooting' "$TMP_DIR/guide_whepfrom.md"; then
        echo "llms-txt-check: deployed LLM files and key tool pages are available at $BASE_URL"
        exit 0
    fi

    if ((SECONDS >= deadline)); then
        echo "llms-txt-check: deployed files were not ready within ${TIMEOUT}s at $BASE_URL" >&2
        exit 1
    fi
    sleep "$INTERVAL"
done
