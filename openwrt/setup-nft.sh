#!/bin/sh
# Load wrtg nft DNAT rules via CLI (nft -f fails on some OpenWrt builds).
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$ROOT/lib.sh"

load_config

ELEMENTS="$(nft_cidr_inline)"
[ -n "$ELEMENTS" ] || {
	echo "wrtg: no valid Telegram CIDRs; keeping current nft table" >&2
	exit 1
}

RULES="$(mktemp)"
trap 'rm -f "$RULES"' EXIT HUP INT TERM

if nft list table inet tg_tproxy >/dev/null 2>&1; then
	echo "delete table inet tg_tproxy" >> "$RULES"
fi
cat >> "$RULES" <<EOF
add table inet tg_tproxy
add set inet tg_tproxy telegram_cidr { type ipv4_addr; flags interval; }
add element inet tg_tproxy telegram_cidr { $ELEMENTS }
add chain inet tg_tproxy prerouting { type nat hook prerouting priority dstnat; policy accept; }
EOF

# Hosts to leave alone, matched before the DNAT rules below.
#
# A client that runs its own DPI-bypass sends decoy ClientHellos (fake SNI, low
# TTL, bogus TCP-MD5) meant to die in transit. wrtg terminates TCP one hop away,
# so those decoys arrive as ordinary payload and get relayed to Telegram, which
# never answers them — the client then retries every few seconds forever.
# Excluding such a host lets its own bypass work end to end.
for SRC in $WRTG_SKIP_SRC; do
	SRC=$(printf '%s' "$SRC" | tr -d '\r')
	[ -n "$SRC" ] || continue
	# The value is interpolated straight into an nft rule, so accept only a
	# bare IPv4 address or CIDR — anything else is rejected rather than
	# smuggled into the ruleset. Octets are range-checked too: nft validates
	# the batch as a whole, so one typo like 192.168.1.300 would otherwise
	# fail the entire load and leave the router with no DNAT at all.
	_octet='(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])'
	if ! printf '%s' "$SRC" | grep -qE "^($_octet\.){3}$_octet(/([0-9]|[12][0-9]|3[0-2]))?$"; then
		echo "wrtg: ignoring invalid WRTG_SKIP_SRC entry: $SRC" >&2
		continue
	fi
	echo "add rule inet tg_tproxy prerouting meta nfproto ipv4 ip saddr $SRC return" >> "$RULES"
done

# LAN_IF may be a space-separated list (e.g. br-lan + NetBird wt0 exit-node);
# a single iifname "$LAN_IF" with several names is not valid nft syntax.
for IF in $LAN_IF; do
	IF=$(printf '%s' "$IF" | tr -d '\r')
	[ -n "$IF" ] || continue
	echo "add rule inet tg_tproxy prerouting iifname \"$IF\" meta nfproto ipv4 ip daddr @telegram_cidr tcp dport { 80, 443, 5222 } dnat ip to $ROUTER_IP:$LISTEN_PORT" >> "$RULES"
done

# nft batches are atomic: a validation/apply error leaves the previous table intact.
nft -c -f "$RULES"
nft -f "$RULES"

echo "wrtg DNAT loaded -> $ROUTER_IP:$LISTEN_PORT (ports 80,443,5222 on $LAN_IF)"
