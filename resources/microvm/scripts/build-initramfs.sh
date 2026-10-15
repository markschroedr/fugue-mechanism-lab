#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ASSETS="$ROOT/assets"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

tar -xzf "$ASSETS/alpine-minirootfs.tar.gz" -C "$stage"
install -m 0755 "$ROOT/guest/init" "$stage/init"
install -m 0755 "$ROOT/guest/home_snapshot.py" "$stage/home_snapshot.py"
mkdir -p "$stage/newroot" "$stage/control"
(cd "$stage" && find . -print0 | cpio --null -o -H newc -R 0:0 2>/dev/null) \
  | gzip -9 > "$ASSETS/initramfs.gz"
echo "sandbox: built initramfs"
