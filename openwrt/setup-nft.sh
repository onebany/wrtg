#!/bin/sh
# Load wrtg nft DNAT rules via CLI (nft -f fails on some OpenWrt builds).
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$ROOT/lib.sh"

load_config

ELEMENTS="$(nft_cidr_elements)"

nft delete table inet tg_tproxy 2>/dev/null || true
nft add table inet tg_tproxy
nft add set inet tg_tproxy telegram_cidr "{ type ipv4_addr; flags interval; elements = {
$ELEMENTS
}; }"
nft add chain inet tg_tproxy prerouting "{ type nat hook prerouting priority dstnat; policy accept; }"
nft add rule inet tg_tproxy prerouting \
	iifname "$LAN_IF" meta nfproto ipv4 ip daddr @telegram_cidr tcp dport 443 \
	dnat ip to "$ROUTER_IP:$LISTEN_PORT"
nft add rule inet tg_tproxy prerouting \
	iifname "$LAN_IF" meta nfproto ipv4 ip daddr @telegram_cidr tcp dport 80 \
	dnat ip to "$ROUTER_IP:$LISTEN_PORT"
nft add rule inet tg_tproxy prerouting \
	iifname "$LAN_IF" meta nfproto ipv4 ip daddr @telegram_cidr tcp dport 5222 \
	dnat ip to "$ROUTER_IP:$LISTEN_PORT"

calls_zapret_bypass_apply 2>/dev/null || true

echo "wrtg DNAT loaded -> $ROUTER_IP:$LISTEN_PORT (ports 80,443,5222 on $LAN_IF)"
