#!/usr/bin/env bash
# Pins NAVIGATOR_REF to the sibling checkout's HEAD: the navigator commit the
# release was qualified against. The release workflow refuses a pin that is not
# green on navigator's own CI.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -n "$(git -C ../navigator status --porcelain)" ]; then
    echo "warning: ../navigator has uncommitted changes the pin cannot capture" >&2
fi
ref=$(git -C ../navigator rev-parse HEAD)
echo "$ref" > NAVIGATOR_REF
echo "pinned navigator $ref"
