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
	mipsel) TARGET=mipsel-unknown-linux-musl; OUT="$DIST/wrtg-linux-mipsel" ;;
	*) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

# mipsel (e.g. MT7621/24kc, mips32r2) is a tier-3 Rust target: no prebuilt std,
# so it needs nightly + -Zbuild-std and a mipsel musl cross-gcc (musl.cc).
if [ "$ARCH" = "mipsel" ]; then
	MIPSEL_CC="${MIPSEL_CC:-mipsel-linux-musl-gcc}"
	PATCHELF="${PATCHELF:-patchelf}"
	command -v "$MIPSEL_CC" >/dev/null 2>&1 || {
		echo "mipsel cross-compiler not found: $MIPSEL_CC" >&2
		echo "install https://musl.cc/mipsel-linux-musl-cross.tgz or set MIPSEL_CC" >&2
		exit 1
	}
	mkdir -p "$DIST"
	echo "Building wrtg for $TARGET (nightly -Zbuild-std) -> $OUT"
	export CARGO_TARGET_MIPSEL_UNKNOWN_LINUX_MUSL_LINKER="$MIPSEL_CC"
	export CC_mipsel_unknown_linux_musl="$MIPSEL_CC"
	export CFLAGS_mipsel_unknown_linux_musl="${MIPSEL_CFLAGS:--march=mips32r2 -mtune=24kc}"
	# panic=immediate-abort: stock OpenWrt has no libgcc_s.so.1, and the unwinder
	# is the only hard consumer of it.
	export RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort -Ctarget-cpu=mips32r2"
	(
		cd "$ROOT"
		cargo +nightly build -Zbuild-std --release -p wrtg --target "$TARGET"
	)
	cp "$ROOT/target/$TARGET/release/wrtg" "$OUT"
	# gcc toolchains leave a spurious DT_NEEDED on libgcc_s.so.1 (absent on stock
	# OpenWrt) via crtbegin's weak __register_frame_info@GLIBC_2.0 refs; no strong
	# symbols from it are used (verified). zig cc builds don't have this problem.
	if readelf -d "$OUT" | grep -q "libgcc_s"; then
		command -v "$PATCHELF" >/dev/null 2>&1 || {
			echo "binary needs libgcc_s.so.1 and patchelf not found: $PATCHELF" >&2
			exit 1
		}
		"$PATCHELF" --remove-needed libgcc_s.so.1 "$OUT"
	fi
	chmod +x "$OUT"
	echo "Built $OUT"
	exit 0
fi

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
