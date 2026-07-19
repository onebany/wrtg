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
