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

mkdir -p "$DIST"
echo "Building wrtg for $TARGET -> $OUT"

if command -v cargo-zigbuild >/dev/null 2>&1 && command -v zig >/dev/null 2>&1; then
	(
		cd "$ROOT"
		cargo zigbuild --release -p wrtg --target "$TARGET"
	)
	cp "$ROOT/target/$TARGET/release/wrtg" "$OUT"
elif [ "$TARGET" = "x86_64-unknown-linux-musl" ] && command -v musl-gcc >/dev/null 2>&1; then
	export CC_x86_64_unknown_linux_musl=musl-gcc
	export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
	(
		cd "$ROOT"
		cargo build --release -p wrtg --target "$TARGET"
	)
	cp "$ROOT/target/$TARGET/release/wrtg" "$OUT"
elif [ "$TARGET" = "x86_64-unknown-linux-musl" ] &&
	command -v docker >/dev/null 2>&1 &&
	docker info >/dev/null 2>&1; then
	image="wrtg-build-local:${VERSION:-dev}"
	container=
	trap '[ -n "$container" ] && docker rm -f "$container" >/dev/null 2>&1 || true' EXIT HUP INT TERM
	docker build --target build -t "$image" -f "$ROOT/docker/Dockerfile" "$ROOT"
	container="$(docker create "$image")"
	docker cp "$container:/src/target/$TARGET/release/wrtg" "$OUT"
	docker rm "$container" >/dev/null
	container=
	trap - EXIT HUP INT TERM
else
	echo "no musl cross-linker for $TARGET" >&2
	echo "install zig + cargo-zigbuild (recommended), musl-tools (amd64), or Docker (amd64)" >&2
	exit 1
fi

chmod +x "$OUT"
echo "Built $OUT"
