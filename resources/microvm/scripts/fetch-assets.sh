#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ASSETS="$ROOT/assets"
KATA_VERSION="3.31.0"
ALPINE_VERSION="3.22.4"
KERNEL_SHA256="a44d663f4ddad20a35527a3578fadef9beb23c1e5cb720e85d6928d6de70d3a1"
ALPINE_SHA256="fc11cf987b37b2e57969cea7e0b8df0777e572d41fe20731630c1e926a8a07a2"

mkdir -p "$ASSETS"

check() {
  local expected="$1" path="$2"
  [[ -f "$path" ]] && [[ "$(shasum -a 256 "$path" | awk '{print $1}')" == "$expected" ]]
}

if ! check "$ALPINE_SHA256" "$ASSETS/alpine-minirootfs.tar.gz"; then
  rm -f "$ASSETS/alpine-minirootfs.tar.gz"
  curl -fL \
    "https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/alpine-minirootfs-${ALPINE_VERSION}-aarch64.tar.gz" \
    -o "$ASSETS/alpine-minirootfs.tar.gz"
  check "$ALPINE_SHA256" "$ASSETS/alpine-minirootfs.tar.gz"
fi

if ! check "$KERNEL_SHA256" "$ASSETS/vmlinux"; then
  archive="$ASSETS/kata-static.tar.zst"
  stage="$(mktemp -d)"
  trap 'rm -rf "$stage" "$archive"' EXIT
  curl -fL \
    "https://github.com/kata-containers/kata-containers/releases/download/${KATA_VERSION}/kata-static-${KATA_VERSION}-arm64.tar.zst" \
    -o "$archive"
  zstd -dc "$archive" | tar -x -C "$stage" 'opt/kata/share/kata-containers/'
  kernel=""
  while IFS= read -r candidate; do
    if check "$KERNEL_SHA256" "$candidate"; then
      kernel="$candidate"
      break
    fi
  done < <(find "$stage/opt/kata/share/kata-containers" -type f -name 'vmlinux*')
  [[ -n "$kernel" ]]
  cp "$kernel" "$ASSETS/vmlinux"
  check "$KERNEL_SHA256" "$ASSETS/vmlinux"
fi

echo "sandbox: verified boot assets"
