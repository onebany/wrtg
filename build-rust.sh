#!/bin/sh
# Cross-compile wrtg (Rust) for OpenWrt targets.
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
DIST="$ROOT/dist"
ARCH="${1:-amd64}"

case "$ARCH" in
	amd64|x86_64) TARGET=x86_64-unknown-linux-musl; OUT="$DIST/wrtg-linux-amd64" ;;
	arm64|aarch64) TARGET=aarch64-unknown-linux-musl; OUT="$DIST/wrtg-linux-arm64" ;;
	arm|armv7) TARGET=armv7-unknown-linux-musleabihf; OUT="$DIST/wrtg-linux-arm" ;;
	*) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

if ! command -v rustup >/dev/null 2>&1; then
	echo "rustup not found; install Rust: https://rustup.rs" >&2
	exit 1
fi

rustup target add "$TARGET" 2>/dev/null || true

# musl cross-linker hints (optional; rustup targets often work out of the box on Linux)
case "$TARGET" in
	x86_64-unknown-linux-musl) ;;
	aarch64-unknown-linux-musl) ;;
	armv7-unknown-linux-musleabihf)
		if command -v arm-linux-gnueabihf-gcc >/dev/null 2>&1; then
			export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER=arm-linux-gnueabihf-gcc
		fi
		;;
esac

mkdir -p "$DIST"
echo "Building wrtg for $TARGET -> $OUT"
(
	cd "$ROOT"
	cargo build --release -p wrtg --target "$TARGET"
)
cp "$ROOT/target/$TARGET/release/wrtg" "$OUT"
chmod +x "$OUT"
echo "Built $OUT"
