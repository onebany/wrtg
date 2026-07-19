#!/bin/sh
# wrtg one-line installer for OpenWrt.
#
# Downloads a release bundle (preferred) or assembles from release binary +
# source archive, then runs install.sh. No git, no Rust, no build required.
#
#   wget -qO- https://raw.githubusercontent.com/onebany/wrtg/main/bootstrap.sh | sh
#
# Options (env):
#   VER=v0.5.5        Install a specific release instead of the latest
#   WRTG_REPO=o/r     GitHub repo to install from (default: onebany/wrtg)
#   WRTG_BASE_URL=    Install from a self-hosted Gitea host instead of GitHub
#                     (Gitea-style API; alias: WRTG_RELEASE_URL)
#   WRTG_INSECURE=1   Allow unverified installs: downgrade a missing sha256sum
#                     tool / SHA256SUMS to a warning AND permit the unverified
#                     binary+source fallback (not recommended)
#   ASSUME_YES=1      Non-interactive (accept config defaults)
#   plus any install.sh option: SKIP_LUCI=1, FRONT_IP=, CF_WORKER_DOMAIN=, ...

set -e

BASE="${WRTG_BASE_URL:-${WRTG_RELEASE_URL:-}}"
WRTG_REPO="${WRTG_REPO:-onebany/wrtg}"
VER="${VER:-latest}"
BUNDLE="wrtg-openwrt.tar.gz"
TMP="$(mktemp -d /tmp/wrtg-install.XXXXXX)" || { echo "wrtg: mktemp failed" >&2; exit 1; }
trap 'rm -rf "$TMP"' EXIT HUP INT TERM
# Checksum verification is fail-closed. Set WRTG_INSECURE=1 to downgrade a
# missing sha256sum tool / missing SHA256SUMS to a warning (not recommended).
INSECURE="${WRTG_INSECURE:-0}"

err() { echo "wrtg: $*" >&2; exit 1; }
warn() { echo "wrtg: $*" >&2; }

fetch() { # url dest
	if command -v curl >/dev/null 2>&1; then curl -fsSL --max-time 15 "$1" -o "$2"
	elif command -v wget >/dev/null 2>&1; then wget -q -T 15 -O "$2" "$1"
	else err "need curl or wget"; fi
}

fetch_optional() { # url dest — returns 0 on success, 1 on 404/missing
	if command -v curl >/dev/null 2>&1; then curl -fsSL --max-time 15 "$1" -o "$2"
	elif command -v wget >/dev/null 2>&1; then wget -q -T 15 -O "$2" "$1"
	else return 1; fi
}

github_mode() { [ -z "$BASE" ]; } # GitHub unless a custom base URL is given

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

resolve_latest_github_atom() {
	# Resolve the latest tag from the releases atom feed instead of
	# api.github.com. The REST API rate-limits unauthenticated requests to
	# 60/hour per IP, so shared/CGNAT ISP addresses often get HTTP 403 here;
	# the atom feed is served from github.com and is not subject to that limit.
	_atom="https://github.com/${WRTG_REPO:-onebany/wrtg}/releases.atom"
	fetch_optional "$_atom" "$TMP/releases.atom" || return 1
	_tag="$(grep -o 'releases/tag/[^"]*' "$TMP/releases.atom" | head -n1 | sed 's#.*releases/tag/##' | tr -d '\r')"
	[ -n "$_tag" ] || return 1
	printf '%s' "$_tag"
}

resolve_latest_ver() {
	_rb="$(release_base)"
	if github_mode; then
		# Prefer the atom feed (no API rate limit); fall back to the REST API.
		_tag="$(resolve_latest_github_atom)"
		[ -n "$_tag" ] && { printf '%s' "$_tag"; return 0; }
		_api="https://api.github.com/repos/${WRTG_REPO:-onebany/wrtg}/releases/latest"
	else
		_api="$(gitea_api_base)/releases/latest"
	fi
	if ! fetch_optional "$_api" "$TMP/latest.json"; then
		if github_mode; then
			err "cannot resolve latest release (atom feed and API both failed) — pass an explicit VER=vX.Y.Z"
		else
			err "cannot resolve latest release from $_api"
		fi
	fi
	_tag="$(parse_tag_name "$TMP/latest.json" | tr -d '\r')"
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

# Verify a downloaded file against the release SHA256SUMS. Fails closed: a
# missing sha256sum tool or a missing/incomplete SHA256SUMS aborts the install
# (this is a root-level installer). WRTG_INSECURE=1 downgrades those two cases
# to a warning; a checksum *mismatch* always aborts regardless.
verify_checksum() { # ver file_path
	_ver="$1"
	_file_path="$2"
	_file="$(basename "$_file_path")"

	if ! command -v sha256sum >/dev/null 2>&1; then
		[ "$INSECURE" = "1" ] || err "sha256sum not found — cannot verify $_file; install it or re-run with WRTG_INSECURE=1"
		warn "sha256sum not found — skipping checksum verification (WRTG_INSECURE=1)"
		return 0
	fi

	if [ ! -f "$TMP/SHA256SUMS" ]; then
		_sum_url="$(bundle_url "$_ver" SHA256SUMS)"
		if ! fetch_optional "$_sum_url" "$TMP/SHA256SUMS"; then
			[ "$INSECURE" = "1" ] || err "SHA256SUMS not found at $_sum_url — refusing to install unverified $_file (set WRTG_INSECURE=1 to override)"
			warn "SHA256SUMS not found — skipping checksum verification (WRTG_INSECURE=1)"
			return 0
		fi
	fi

	_expected="$(awk -v f="$_file" '$2 == f { print $1; exit }' "$TMP/SHA256SUMS")"
	[ -n "$_expected" ] || err "$_file missing from SHA256SUMS"
	_actual="$(sha256sum "$_file_path" | awk '{print $1}')"
	[ "$_actual" = "$_expected" ] || err "$_file checksum mismatch (expected $_expected, got $_actual)"
	echo "wrtg: verified $_file (sha256 ok)"
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
	verify_checksum "$_ver" "$TMP/$BUNDLE"
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
	verify_checksum "$_ver" "$TMP/$_bin"
	chmod +x "$TMP/$_bin"

	fetch "$_arch_url" "$TMP/src.tar.gz" || err "source archive download failed ($_arch_url)"
	mkdir -p "$TMP/src"
	tar -xzf "$TMP/src.tar.gz" -C "$TMP/src" || err "source extract failed"
	_dir="$(find_install_dir "$TMP/src")" || err "install.sh not found in source archive"
	mkdir -p "$_dir/dist"
	cp "$TMP/$_bin" "$_dir/dist/$_bin"
	chmod 755 "$_dir/dist/$_bin"

	echo "wrtg: installing ..."
	SKIP_BUILD=1 sh "$_dir/install.sh"
}

# ── main ─────────────────────────────────────────────────────────────────────
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
	# The fallback runs install.sh from an unverified source archive as root
	# (only the binary is sha256-verified). Require an explicit opt-in.
	[ "$INSECURE" = "1" ] || err "release bundle unavailable — refusing unverified source fallback (set WRTG_INSECURE=1 to override)"
	warn "release bundle unavailable — falling back to UNVERIFIED binary + source (WRTG_INSECURE=1)"
	install_from_binary "$VER"
fi

rm -rf "$TMP"
