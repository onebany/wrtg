#!/bin/sh
# wrtg one-line installer for OpenWrt.
#
# Downloads a release bundle (preferred) or assembles from release binary +
# source archive, then runs install.sh. No git, no Rust, no build required.
#
#   wget -qO- https://github.com/onebany/wrtg/raw/branch/main/bootstrap.sh | sh
#
# Options (env):
#   VER=v0.5.5        Install a specific release instead of the latest
#   WRTG_BASE_URL=    Override release host (alias: WRTG_RELEASE_URL)
#   WRTG_REPO=o/r     Use GitHub releases instead (e.g. onebany/wrtg)
#   ASSUME_YES=1      Non-interactive (accept config defaults)
#   plus any install.sh option: SKIP_LUCI=1, FRONT_IP=, CF_WORKER_DOMAIN=, ...

set -e

DEFAULT_BASE="https://github.com/onebany/wrtg"
BASE="${WRTG_BASE_URL:-${WRTG_RELEASE_URL:-$DEFAULT_BASE}}"
VER="${VER:-latest}"
BUNDLE="wrtg-openwrt.tar.gz"
TMP="/tmp/wrtg-install"

err() { echo "wrtg: $*" >&2; exit 1; }
warn() { echo "wrtg: $*" >&2; }

fetch() { # url dest
	if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
	elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
	else err "need curl or wget"; fi
}

fetch_optional() { # url dest — returns 0 on success, 1 on 404/missing
	if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
	elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
	else return 1; fi
}

github_mode() { [ -n "${WRTG_REPO:-}" ]; }

release_base() {
	if github_mode; then
		printf 'https://github.com/%s' "${WRTG_REPO:-onebany/wrtg}"
	else
		printf '%s' "${BASE%/}"
	fi
}

gitea_api_base() {
	# https://host/owner/repo -> https://host/api/v1/repos/owner/repo
	_host="${BASE#*://}"
	_host="${_host%%/*}"
	_path="${BASE#*://}"
	_path="${_path#*/}"
	printf 'https://%s/api/v1/repos/%s' "$_host" "$_path"
}

parse_tag_name() { # json_file -> echoes tag
	_tag="$(grep '"tag_name"' "$1" 2>/dev/null | head -n1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"
	[ -n "$_tag" ] && printf '%s' "$_tag"
}

resolve_latest_ver() {
	_rb="$(release_base)"
	if github_mode; then
		_api="https://api.github.com/repos/${WRTG_REPO:-onebany/wrtg}/releases/latest"
	else
		_api="$(gitea_api_base)/releases/latest"
	fi
	fetch "$_api" "$TMP/latest.json" || err "cannot resolve latest release from $_api"
	_tag="$(parse_tag_name "$TMP/latest.json")"
	[ -n "$_tag" ] || err "latest release tag not found in API response"
	printf '%s' "$_tag"
}

normalize_ver() {
	_tag="$1"
	case "$_tag" in
		v*) printf '%s' "$_tag" ;;
		*) printf 'v%s' "$_tag" ;;
	esac
}

bundle_url() { # ver bundle_name
	_rb="$(release_base)"
	_raw="$1"
	_file="$2"
	if github_mode && [ "$_raw" = "latest" ]; then
		printf '%s/releases/latest/download/%s' "$_rb" "$_file"
	else
		_ver="$(normalize_ver "$_raw")"
		printf '%s/releases/download/%s/%s' "$_rb" "$_ver" "$_file"
	fi
}

archive_url() { # ver
	_rb="$(release_base)"
	_ver="$(normalize_ver "$1")"
	if github_mode; then
		printf '%s/archive/refs/tags/%s.tar.gz' "$_rb" "$_ver"
	else
		printf '%s/archive/%s.tar.gz' "$_rb" "$_ver"
	fi
}

detect_arch() {
	case "$(uname -m)" in
		x86_64|amd64) echo amd64 ;;
		aarch64|arm64) echo arm64 ;;
		armv7l|armv7|armv6l) echo arm ;;
		*) return 1 ;;
	esac
}

verify_bundle_checksum() { # bundle_path
	_sum_url="${1%/*}/SHA256SUMS"
	_bundle="$(basename "$1")"
	command -v sha256sum >/dev/null 2>&1 || {
		warn "sha256sum not found — skipping checksum verification"
		return 0
	}
	if ! fetch_optional "$_sum_url" "$TMP/SHA256SUMS"; then
		warn "SHA256SUMS not found — skipping checksum verification"
		return 0
	fi
	_expected="$(awk -v f="$_bundle" '$2 == f { print $1; exit }' "$TMP/SHA256SUMS")"
	[ -n "$_expected" ] || err "$_bundle missing from SHA256SUMS"
	_actual="$(sha256sum "$1" | awk '{print $1}')"
	[ "$_actual" = "$_expected" ] || err "bundle checksum mismatch"
}

find_install_dir() { # root -> echoes dir containing install.sh
	_dir="$1"
	if [ -f "$_dir/install.sh" ]; then
		printf '%s' "$_dir"
		return
	fi
	_found="$(find "$_dir" -name install.sh -print 2>/dev/null | head -n1)"
	[ -n "$_found" ] || return 1
	dirname "$_found"
}

install_from_bundle() { # ver
	_ver="$1"
	_url="$(bundle_url "$_ver" "$BUNDLE")"
	echo "wrtg: downloading bundle $_ver ..."
	fetch "$_url" "$TMP/$BUNDLE" || return 1
	verify_bundle_checksum "$TMP/$BUNDLE"
	tar -xzf "$TMP/$BUNDLE" -C "$TMP" || err "extract failed"
	_dir="$(find_install_dir "$TMP")" || err "install.sh not found in bundle"
	echo "wrtg: installing ..."
	SKIP_BUILD=1 sh "$_dir/install.sh"
}

install_from_binary() { # ver
	_ver="$1"
	_arch="$(detect_arch)" || err "unsupported CPU: $(uname -m)"
	_bin="wrtg-linux-$_arch"
	_bin_url="$(bundle_url "$_ver" "$_bin")"
	_arch_url="$(archive_url "$_ver")"

	echo "wrtg: bundle not found — using release binary + source ($_arch) ..."
	fetch "$_bin_url" "$TMP/$_bin" || err "binary download failed ($_bin_url)"
	chmod +x "$TMP/$_bin"

	fetch "$_arch_url" "$TMP/src.tar.gz" || err "source archive download failed ($_arch_url)"
	mkdir -p "$TMP/src"
	tar -xzf "$TMP/src.tar.gz" -C "$TMP/src" || err "source extract failed"
	_dir="$(find_install_dir "$TMP/src")" || err "install.sh not found in source archive"
	mkdir -p "$_dir/dist"
	install -m 755 "$TMP/$_bin" "$_dir/dist/$_bin"

	echo "wrtg: installing ..."
	SKIP_BUILD=1 sh "$_dir/install.sh"
}

# ── main ─────────────────────────────────────────────────────────────────────
rm -rf "$TMP"
mkdir -p "$TMP"

if [ "$VER" = "latest" ]; then
	VER="$(resolve_latest_ver)"
	echo "wrtg: latest release is $VER"
fi

_src="$(release_base)"
if github_mode; then
	echo "wrtg: GitHub releases ($WRTG_REPO) $VER"
else
	echo "wrtg: Gitea releases ($_src) $VER"
fi

if install_from_bundle "$VER"; then
	:
else
	warn "release bundle unavailable — falling back to binary + source"
	install_from_binary "$VER"
fi

rm -rf "$TMP"
