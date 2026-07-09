#!/bin/sh
# Refresh Telegram CIDR list from official source + local extras, then reload nft.

set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$ROOT/lib.sh"

load_config

mkdir -p "$(dirname "$CIDR_FILE")"
TMP="$(mktemp)"
OFFICIAL="$TMP.official"
VALID_OFFICIAL="$TMP.valid-official"
trap 'rm -f "$TMP" "$OFFICIAL" "$VALID_OFFICIAL"' EXIT HUP INT TERM

if command -v curl >/dev/null 2>&1; then
	curl -fsSL --connect-timeout 15 --max-time 60 "$CIDR_URL" > "$OFFICIAL" || true
elif command -v wget >/dev/null 2>&1; then
	wget -qO "$OFFICIAL" "$CIDR_URL" || true
else
	echo "update-cidr: curl or wget required" >&2
	exit 1
fi

valid_ipv4_cidrs < "$OFFICIAL" > "$VALID_OFFICIAL"
OFFICIAL_COUNT="$(wc -l < "$VALID_OFFICIAL" | tr -d ' ')"
{
	if [ "${OFFICIAL_COUNT:-0}" -ge 5 ]; then
		cat "$VALID_OFFICIAL"
	else
		echo "update-cidr: invalid/short official response; using built-in defaults" >&2
		default_cidrs
	fi
	if [ -f "$CIDR_EXTRA" ]; then
		valid_ipv4_cidrs < "$CIDR_EXTRA"
	fi
} | sort -u > "$TMP"

COUNT="$(wc -l < "$TMP" | tr -d ' ')"
[ "${COUNT:-0}" -ge 5 ] || {
	echo "update-cidr: refusing to install only ${COUNT:-0} valid networks" >&2
	exit 1
}

# Validate and atomically apply nft using the candidate file first.
WRTG_CIDR_FILE="$TMP" "$ROOT/setup-nft.sh"
mv "$TMP" "$CIDR_FILE"
trap - EXIT HUP INT TERM
rm -f "$OFFICIAL" "$VALID_OFFICIAL"
echo "Telegram CIDR set updated ($COUNT networks) in $CIDR_FILE"
