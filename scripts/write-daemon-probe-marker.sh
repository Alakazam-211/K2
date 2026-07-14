#!/bin/sh
set -eu

app="${1:-}"
if [ -z "$app" ]; then
    echo "Usage: write-daemon-probe-marker.sh <K2.app>" >&2
    exit 2
fi

daemon="$app/Contents/MacOS/k2-daemon"
marker="$app/Contents/Resources/k2-daemon.artifact-probe-v1.sha256"
if [ ! -x "$daemon" ]; then
    echo "write-daemon-probe-marker: daemon not found: $daemon" >&2
    exit 1
fi

mkdir -p "$(dirname "$marker")"
shasum -a 256 "$daemon" | awk '{print $1}' > "$marker"
