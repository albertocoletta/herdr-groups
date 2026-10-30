#!/usr/bin/env bash
set -euo pipefail

NAME="herdr-groups"
REPO="albertocoletta/herdr-groups"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="$ROOT/bin"
VERSION="$(grep -m1 '^version' "$ROOT/herdr-plugin.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
TAG="v${VERSION}"

os="$(uname -s)"
arch="$(uname -m)"
case "$os-$arch" in
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-musl" ;;
  Linux-x86_64) target="x86_64-unknown-linux-musl" ;;
  *)
    echo "groups: no prebuilt binary for $os-$arch; build with 'cargo build --release' and copy target/release/$NAME into bin/" >&2
    exit 1
    ;;
esac

archive="${NAME}-${target}.tar.gz"
checksum="${NAME}-${target}.sha256"
base="https://github.com/${REPO}/releases/download/${TAG}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

download() {
  for _ in 1 2 3 4 5 6; do
    curl -fsSL "$1" -o "$2" && return
    sleep 3
  done
  return 1
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

echo "groups: downloading $archive ($TAG)"
download "$base/$archive" "$tmp/$archive"
download "$base/$checksum" "$tmp/$checksum"

expected="$(awk '{print $1}' "$tmp/$checksum")"
actual="$(sha256 "$tmp/$archive")"
if [ "$expected" != "$actual" ]; then
  echo "groups: checksum mismatch for $archive (expected $expected, got $actual)" >&2
  exit 1
fi

mkdir -p "$BIN_DIR"
tar -xzf "$tmp/$archive" -C "$tmp"
install -m 0755 "$tmp/$NAME" "$BIN_DIR/$NAME"
echo "groups: installed $BIN_DIR/$NAME"
