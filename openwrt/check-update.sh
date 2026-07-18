#!/bin/sh
# Check for / apply wrtg updates from GitHub releases (same path as bootstrap.sh).
#
# Usage:
#   /etc/wrtg/check-update.sh check          # print CURRENT / LATEST / AVAILABLE
#   /etc/wrtg/check-update.sh update [VER]   # install latest (or VER); preserves /etc/wrtg/config
#
# Env:
#   WRTG_REPO=owner/repo   (default: onebany/wrtg)
#   WRTG_INSECURE=1        skip checksum when sha256sum/SHA256SUMS missing (not recommended)

set -e

WRTG_REPO="${WRTG_REPO:-onebany/wrtg}"
VERSION_FILE="${WRTG_VERSION_FILE:-/etc/wrtg/version}"
BUNDLE="wrtg-openwrt.tar.gz"
TMP="/tmp/wrtg-update"
INSECURE="${WRTG_INSECURE:-0}"
CMD="${1:-check}"
REQ_VER="${2:-}"

err() { echo "wrtg: $*" >&2; exit 1; }
warn() { echo "wrtg: $*" >&2; }

fetch() {
	if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
	elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
	else err "need curl or wget"; fi
}

fetch_optional() {
	if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
	elif command -v wget >/dev/null 2>&1; then wget -qO "$2" "$1"
	else return 1; fi
}

normalize_ver() {
	_t="$1"
	_t="${_t#v}"
	_t="$(printf '%s' "$_t" | tr -d '\r\n[:space:]')"
	[ -n "$_t" ] || return 1
	printf '%s' "$_t"
}

normalize_tag() {
	_t="$(normalize_ver "$1")" || return 1
	printf 'v%s' "$_t"
}

current_ver() {
	if [ -f "$VERSION_FILE" ]; then
		normalize_ver "$(cat "$VERSION_FILE")" || printf '0'
	else
		printf '0'
	fi
}

resolve_latest_atom() {
	_atom="https://github.com/${WRTG_REPO}/releases.atom"
	fetch_optional "$_atom" "$TMP/releases.atom" || return 1
	_tag="$(grep -o 'releases/tag/[^"]*' "$TMP/releases.atom" | head -n1 | sed 's#.*releases/tag/##' | tr -d '\r')"
	[ -n "$_tag" ] || return 1
	normalize_tag "$_tag"
}

resolve_latest_api() {
	_api="https://api.github.com/repos/${WRTG_REPO}/releases/latest"
	fetch_optional "$_api" "$TMP/latest.json" || return 1
	_tag="$(grep '"tag_name"' "$TMP/latest.json" 2>/dev/null | head -n1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"
	[ -n "$_tag" ] || return 1
	normalize_tag "$_tag"
}

resolve_latest() {
	mkdir -p "$TMP"
	_tag="$(resolve_latest_atom 2>/dev/null)" || _tag=""
	[ -n "$_tag" ] || _tag="$(resolve_latest_api 2>/dev/null)" || _tag=""
	[ -n "$_tag" ] || err "cannot resolve latest release (atom feed and API both failed)"
	printf '%s' "$_tag"
}

# Returns 0 if a > b (semver-ish via sort -V), 1 otherwise.
ver_gt() {
	_a="$(normalize_ver "$1")" || return 1
	_b="$(normalize_ver "$2")" || return 1
	[ "$_a" = "$_b" ] && return 1
	_top="$(printf '%s\n%s\n' "$_a" "$_b" | sort -V | tail -n1)"
	[ "$_top" = "$_a" ]
}

bundle_url() {
	_ver="$(normalize_tag "$1")" || return 1
	printf 'https://github.com/%s/releases/download/%s/%s' "$WRTG_REPO" "$_ver" "$2"
}

verify_checksum() {
	_ver="$1"
	_file_path="$2"
	_file="$(basename "$_file_path")"

	if ! command -v sha256sum >/dev/null 2>&1; then
		[ "$INSECURE" = "1" ] || err "sha256sum not found — cannot verify $_file; set WRTG_INSECURE=1 to override"
		warn "sha256sum not found — skipping checksum (WRTG_INSECURE=1)"
		return 0
	fi

	if [ ! -f "$TMP/SHA256SUMS" ]; then
		_sum_url="$(bundle_url "$_ver" SHA256SUMS)"
		if ! fetch_optional "$_sum_url" "$TMP/SHA256SUMS"; then
			[ "$INSECURE" = "1" ] || err "SHA256SUMS not found — refusing unverified $_file (set WRTG_INSECURE=1 to override)"
			warn "SHA256SUMS not found — skipping checksum (WRTG_INSECURE=1)"
			return 0
		fi
	fi

	_expected="$(awk -v f="$_file" '$2 == f { print $1; exit }' "$TMP/SHA256SUMS")"
	[ -n "$_expected" ] || err "$_file missing from SHA256SUMS"
	_actual="$(sha256sum "$_file_path" | awk '{print $1}')"
	[ "$_actual" = "$_expected" ] || err "$_file checksum mismatch"
	echo "wrtg: verified $_file (sha256 ok)"
}

find_install_dir() {
	_dir="$1"
	if [ -f "$_dir/install.sh" ]; then
		printf '%s' "$_dir"
		return 0
	fi
	_found="$(find "$_dir" -name install.sh -print 2>/dev/null | head -n1)"
	[ -n "$_found" ] || return 1
	dirname "$_found"
}

do_check() {
	_cur="$(current_ver)"
	_latest="$(resolve_latest)"
	_lat_n="$(normalize_ver "$_latest")"
	_avail=0
	if ver_gt "$_lat_n" "$_cur"; then
		_avail=1
	fi
	# Machine-readable lines for LuCI / scripts.
	echo "CURRENT=$_cur"
	echo "LATEST=$_lat_n"
	echo "AVAILABLE=$_avail"
	if [ "$_avail" = "1" ]; then
		echo "STATUS=update_available"
		echo "wrtg: update available: $_cur -> $_lat_n"
	elif [ "$_cur" = "$_lat_n" ]; then
		echo "STATUS=up_to_date"
		echo "wrtg: up to date ($_cur)"
	else
		# Installed newer than GitHub latest (dev / pre-release deploy).
		echo "STATUS=newer_local"
		echo "wrtg: installed $_cur is newer than latest release $_lat_n"
	fi
	return 0
}

do_update() {
	_cur="$(current_ver)"
	if [ -n "$REQ_VER" ]; then
		_target="$(normalize_tag "$REQ_VER")" || err "invalid version: $REQ_VER"
	else
		_target="$(resolve_latest)"
	fi
	_tgt_n="$(normalize_ver "$_target")"

	if [ "$_tgt_n" = "$_cur" ]; then
		echo "CURRENT=$_cur"
		echo "LATEST=$_tgt_n"
		echo "AVAILABLE=0"
		echo "STATUS=up_to_date"
		echo "wrtg: already at $_cur — nothing to update"
		return 0
	fi

	if [ -z "$REQ_VER" ] && ! ver_gt "$_tgt_n" "$_cur"; then
		echo "CURRENT=$_cur"
		echo "LATEST=$_tgt_n"
		echo "AVAILABLE=0"
		echo "STATUS=newer_local"
		echo "wrtg: refusing to downgrade $_cur -> $_tgt_n (pass an explicit VER to force)"
		return 1
	fi

	# Preserve config explicitly (install.sh already keeps it; this is defense in depth).
	_cfg_bak=""
	if [ -f /etc/wrtg/config ]; then
		_cfg_bak="$TMP/config.preserve"
		mkdir -p "$TMP"
		cp -a /etc/wrtg/config "$_cfg_bak"
	fi

	rm -rf "$TMP/extract"
	mkdir -p "$TMP" "$TMP/extract"
	_url="$(bundle_url "$_target" "$BUNDLE")"
	echo "wrtg: downloading $_target ..."
	fetch "$_url" "$TMP/$BUNDLE" || err "bundle download failed ($_url)"
	verify_checksum "$_target" "$TMP/$BUNDLE"
	tar -xzf "$TMP/$BUNDLE" -C "$TMP/extract" || err "extract failed"
	_dir="$(find_install_dir "$TMP/extract")" || err "install.sh not found in bundle"

	echo "wrtg: installing $_target (preserving /etc/wrtg/config) ..."
	ASSUME_YES=1 SKIP_BUILD=1 sh "$_dir/install.sh" || err "install.sh failed"

	if [ -n "$_cfg_bak" ] && [ -f "$_cfg_bak" ]; then
		cp -a "$_cfg_bak" /etc/wrtg/config
		chmod 600 /etc/wrtg/config
	fi

	# install.sh restarts the service; ensure it is up.
	if [ -x /etc/init.d/wrtg ]; then
		if pidof wrtg >/dev/null 2>&1; then
			:
		else
			/etc/init.d/wrtg start >/dev/null 2>&1 || true
		fi
	fi

	_new="$(current_ver)"
	echo "CURRENT=$_new"
	echo "LATEST=$_tgt_n"
	echo "AVAILABLE=0"
	echo "STATUS=updated"
	echo "wrtg: updated $_cur -> $_new"
	rm -rf "$TMP"
}

mkdir -p "$TMP"
case "$CMD" in
	check) do_check; exit $? ;;
	update) do_update; exit $? ;;
	*) err "usage: $0 check|update [VER]" ;;
esac
