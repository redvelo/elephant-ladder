#!/usr/bin/env bash
# Pins NAVIGATOR_REF and DICTIONARY_REF to the sibling checkouts' HEADs: the
# commits the release was qualified against. The release workflow refuses a pin
# that is not green on the sibling's own CI.
set -euo pipefail
cd "$(dirname "$0")/.."

pin() {
    local sibling=$1 file=$2
    if [ -n "$(git -C "../$sibling" status --porcelain)" ]; then
        echo "warning: ../$sibling has uncommitted changes the pin cannot capture" >&2
    fi
    local ref
    ref=$(git -C "../$sibling" rev-parse HEAD)
    echo "$ref" > "$file"
    echo "pinned $sibling $ref"
}

pin navigator NAVIGATOR_REF
pin dictionary DICTIONARY_REF
