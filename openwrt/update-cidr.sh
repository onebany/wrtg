#!/bin/sh
# Refresh Telegram CIDR list from official source + local extras, then reload nft.

set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$ROOT/lib.sh"

load_config

mkdir -p "$(dirname "$CIDR_FILE")"
TMP="$(mktemp)"

if command -v curl >/dev/null 2>&1; then
	curl -fsSL --connect-timeout 15 --max-time 60 "$CIDR_URL" > "$TMP.official" || true
elif command -v wget >/dev/null 2>&1; then
	wget -qO "$TMP.official" "$CIDR_URL" || true
else
	echo "update-cidr: curl or wget required" >&2
	exit 1
fi

{
	if [ -s "$TMP.official" ]; then
		grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$' "$TMP.official"
	else
		default_cidrs
	fi
	if [ -f "$CIDR_EXTRA" ]; then
		grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$' "$CIDR_EXTRA"
	fi
} | sort -u > "$TMP"

mv "$TMP" "$CIDR_FILE"
rm -f "$TMP.official"

"$ROOT/setup-nft.sh"
echo "Telegram CIDR set updated ($(wc -l < "$CIDR_FILE" | tr -d ' ') networks) in $CIDR_FILE"
