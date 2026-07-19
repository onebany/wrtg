#!/bin/sh
# OPTIONAL — not part of wrtg core, out of scope for wrtg development.
# Community helper for environments that already run zapret2; wrtg does not depend on zapret.
#
# Telegram voice/video calls: skip zapret nfqueue for call UDP to reflectors.
#
# wrtg covers signaling (TCP MTProto). Media is UDP/WebRTC and is NOT proxied.
# zapret2 queues UDP 3478-3497 (fake STUN) and 50000-65535 (fake QUIC/STUN) which
# breaks ICE to Telegram reflectors (91.108.x.x:3478 STUN, 596-599 TURN, 50k+ P2P).
# UDP 596-599 is not in NFQWS2_PORTS_UDP but 3478 and 50k+ are.
#
# This script inserts nft return rules in zapret2 pre/postnat hooks (no nfqws patch).
# Re-run apply (or setup-nft.sh) after every zapret2 restart — zapret rebuilds its table.
#
# Usage on router:
#   sh zapret-telegram-calls.sh apply
#   sh zapret-telegram-calls.sh revert
#   sh zapret-telegram-calls.sh status

set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$ROOT/lib.sh"

MARKER='wrtg:call-stun-bypass'
UCI=/etc/config/zapret2
CALLS_ZAPRET_BYPASS_FLAG="${WRTG_CALLS_BYPASS_FLAG:-/var/lib/wrtg/calls-zapret-bypass}"
CALLS_NFT_COMMENT='wrtg-calls'

delete_rules() {
	chain="$1"
	nft -a list chain inet zapret2 "$chain" 2>/dev/null |
		grep -F "comment \"$CALLS_NFT_COMMENT\"" |
		sed -n 's/.*# handle \([0-9]*\).*/\1/p' |
		sort -rn | while read -r handle; do
			[ -n "$handle" ] || continue
			nft delete rule inet zapret2 "$chain" handle "$handle" 2>/dev/null || true
		done
}

calls_zapret_bypass_remove() {
	nft list table inet zapret2 >/dev/null 2>&1 || return 0
	delete_rules postnat_hook
	delete_rules prenat_hook
	nft delete set inet zapret2 tg_calls_cidr 2>/dev/null || true
}

calls_zapret_bypass_apply() {
	nft list table inet zapret2 >/dev/null 2>&1 || {
		echo "zapret2 nft table missing" >&2
		return 1
	}
	calls_zapret_bypass_remove
	elements="$(nft_cidr_inline)"
	[ -n "$elements" ] || return 1
	nft add set inet zapret2 tg_calls_cidr '{ type ipv4_addr; flags interval; }'
	nft add element inet zapret2 tg_calls_cidr "{ $elements }"
	# insert (no position) prepends: the return must precede zapret nfqueue rules.
	nft insert rule inet zapret2 postnat_hook \
		oifname @wanif meta nfproto ipv4 ip daddr @tg_calls_cidr \
		udp dport '{ 3478, 596-599, 50000-65535 }' return \
		comment \"$CALLS_NFT_COMMENT\"
	nft insert rule inet zapret2 prenat_hook \
		iifname @wanif meta nfproto ipv4 ip saddr @tg_calls_cidr \
		udp sport '{ 3478, 596-599, 50000-65535 }' return \
		comment \"$CALLS_NFT_COMMENT\"
}

cleanup_legacy_nfqws() {
	[ -f "$UCI" ] || return 0
	grep -qF "$MARKER" "$UCI" 2>/dev/null || return 0
	echo "removing legacy nfqws STUN bypass from $UCI"
	sed -i "s| --new --filter-udp=3478 --ipset=[^ ]* --filter-l7=stun --payload=stun # ${MARKER}||g" "$UCI"
	sed -i "s|--new --filter-udp=3478 --ipset=[^ ]* --filter-l7=stun --payload=stun # ${MARKER} --new --filter-udp=3478-3497|--new --filter-udp=3478-3497|g" "$UCI"
	sed -i "s|^--new --filter-udp=3478 --ipset=[^ ]* --filter-l7=stun --payload=stun # ${MARKER} --new --filter-udp=3478-3497|--filter-udp=3478-3497,19294-19344|g" "$UCI"
	sed -i 's|--new --new --filter-udp=3478|--new --filter-udp=3478|g' "$UCI"
	/etc/init.d/zapret2 restart
}

apply() {
	cleanup_legacy_nfqws
	mkdir -p "$(dirname "$CALLS_ZAPRET_BYPASS_FLAG")"
	touch "$CALLS_ZAPRET_BYPASS_FLAG"
	calls_zapret_bypass_apply
	echo "calls zapret bypass: nft rules applied (flag $CALLS_ZAPRET_BYPASS_FLAG)"
	echo "note: re-run apply after zapret2 restart"
}

revert() {
	rm -f "$CALLS_ZAPRET_BYPASS_FLAG"
	calls_zapret_bypass_remove
	echo "calls zapret bypass: reverted"
}

status() {
	if [ -f "$CALLS_ZAPRET_BYPASS_FLAG" ]; then
		echo "telegram calls zapret bypass: enabled (nft)"
	else
		echo "telegram calls zapret bypass: disabled"
	fi
	if grep -qF "$MARKER" "$UCI" 2>/dev/null; then
		echo "WARNING: legacy nfqws bypass still in $UCI — run apply to remove"
	fi
	nft list chain inet zapret2 postnat_hook 2>/dev/null | grep -F 'wrtg-calls' || \
		echo "nft: no bypass rules in postnat_hook (zapret may need restart + apply)"
	ports="$(grep -E 'NFQWS2_PORTS_UDP' "$UCI" 2>/dev/null | head -1)"
	[ -n "$ports" ] && echo "$ports"
}

case "${1:-status}" in
	apply) apply ;;
	revert) revert ;;
	status) status ;;
	*) echo "usage: $0 {apply|revert|status}"; exit 1 ;;
esac
