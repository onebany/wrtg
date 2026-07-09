#!/bin/sh
# wrtg one-line installer for OpenWrt.
#
# Downloads the latest release bundle (prebuilt musl binaries + service files +
# LuCI app + docs) and runs install.sh. No git, no Rust, no build required.
#
#   wget -qO- https://github.com/OWNER/REPO/raw/main/bootstrap.sh | sh
#
# Options (env):
#   VER=v0.5.0        Install a specific release instead of the latest
#   WRTG_REPO=o/r     Override the GitHub repo (default below)
#   ASSUME_YES=1      Non-interactive (accept config defaults)
#   plus any install.sh option: SKIP_LUCI=1, FRONT_IP=, CF_WORKER_DOMAIN=, ...

set -e

REPO="${WRTG_REPO:-onebany/wrtg}"
VER="${VER:-latest}"
BUNDLE="wrtg-openwrt.tar.gz"
TMP="/tmp/wrtg-install"

err() { echo "wrtg: $*" >&2; exit 1; }

if [ "$VER" = "latest" ]; then
	URL="https://github.com/$REPO/releases/latest/download/$BUNDLE"
else
	URL="https://github.com/$REPO/releases/download/$VER/$BUNDLE"
fi
CHECKSUM_URL="${URL%/*}/SHA256SUMS"

fetch() { # url dest
	if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
	elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
	else err "need curl or wget"; fi
}

echo "wrtg: downloading $VER bundle from $REPO ..."
rm -rf "$TMP"; mkdir -p "$TMP"
fetch "$URL" "$TMP/$BUNDLE" || err "download failed ($URL) - check VER/WRTG_REPO and internet access"
command -v sha256sum >/dev/null 2>&1 || err "sha256sum is required"
fetch "$CHECKSUM_URL" "$TMP/SHA256SUMS" || err "checksum download failed"
EXPECTED="$(awk -v f="$BUNDLE" '$2 == f { print $1; exit }' "$TMP/SHA256SUMS")"
[ -n "$EXPECTED" ] || err "$BUNDLE missing from SHA256SUMS"
ACTUAL="$(sha256sum "$TMP/$BUNDLE" | awk '{print $1}')"
[ "$ACTUAL" = "$EXPECTED" ] || err "bundle checksum mismatch"

tar -xzf "$TMP/$BUNDLE" -C "$TMP" || err "extract failed"

# Bundle may extract into a top-level dir; find install.sh.
DIR="$TMP"
[ -f "$DIR/install.sh" ] || DIR="$(dirname "$(find "$TMP" -name install.sh -print | head -n1)")"
[ -f "$DIR/install.sh" ] || err "install.sh not found in bundle"

echo "wrtg: installing ..."
SKIP_BUILD=1 sh "$DIR/install.sh"
rm -rf "$TMP"
