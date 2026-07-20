#!/bin/sh
set -eu

machine="${1:-$(uname -m)}"

case "$machine" in
    x86_64|amd64)  printf '%s\n' x86_64 ;;
    arm64|aarch64) printf '%s\n' aarch64 ;;
    *)
        echo "macos-native-arch: unsupported architecture: $machine" >&2
        exit 1
        ;;
esac
