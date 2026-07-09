#!/bin/sh
# Shared helpers for wrtg OpenWrt scripts.

CONFIG="${WRTG_CONFIG:-/etc/wrtg/config}"
CIDR_FILE="${WRTG_CIDR_FILE:-/var/lib/wrtg/cidrs.txt}"
CIDR_EXTRA="${WRTG_CIDR_EXTRA:-/etc/wrtg/cidr-extra.txt}"
CALLS_ZAPRET_BYPASS_FLAG="${WRTG_CALLS_BYPASS_FLAG:-/var/lib/wrtg/calls-zapret-bypass}"
CALLS_NFT_COMMENT='wrtg-calls'

load_config() {
	ROUTER_IP=""
	LAN_IF="eth0"
	LISTEN="0.0.0.0:8443"
	FRONT_IP="149.154.167.220"
	WRTG_FRONT_DCS=""
	CF_WORKER_DOMAIN=""
	CF_PROXY_DOMAIN=""
	WRTG_DC_IPS=""
	WRTG_DC_LEARN_FILE=""
	WRTG_DC_IPS_FILE=""
	WRTG_NO_CFPROXY=""
	WRTG_NO_WORKER_PASSTHROUGH=""
	WRTG_CFPROXY_AUTO=""
	WRTG_IP_FAIL_COOLDOWN_SEC=""
	WRTG_WS_POOL_SIZE=""
	WRTG_WS_POOL_TTL_SEC=""
	WRTG_CF_WORKER_POOL_SIZE=""
	WRTG_CF_WORKER_POOL_TTL_SEC=""
	WRTG_WS_BLACKLIST_TTL_SEC=""
	CIDR_URL="https://core.telegram.org/resources/cidr.txt"
	CIDR_UPDATE_HOUR="4"

	if [ -f "$CONFIG" ]; then
		# shellcheck disable=SC1090
		. "$CONFIG"
	fi

	# Windows-edited configs may carry CRLF; strip before use.
	ROUTER_IP=$(printf '%s' "$ROUTER_IP" | tr -d '\r')
	LAN_IF=$(printf '%s' "$LAN_IF" | tr -d '\r')
	LISTEN=$(printf '%s' "$LISTEN" | tr -d '\r')
	FRONT_IP=$(printf '%s' "$FRONT_IP" | tr -d '\r')
	WRTG_FRONT_DCS=$(printf '%s' "$WRTG_FRONT_DCS" | tr -d '\r')
	CF_WORKER_DOMAIN=$(printf '%s' "$CF_WORKER_DOMAIN" | tr -d '\r')
	CF_PROXY_DOMAIN=$(printf '%s' "$CF_PROXY_DOMAIN" | tr -d '\r')
	WRTG_CFPROXY_AUTO=$(printf '%s' "$WRTG_CFPROXY_AUTO" | tr -d '\r')
	WRTG_DC_IPS=$(printf '%s' "$WRTG_DC_IPS" | tr -d '\r')
	WRTG_DC_LEARN_FILE=$(printf '%s' "$WRTG_DC_LEARN_FILE" | tr -d '\r')
	WRTG_DC_IPS_FILE=$(printf '%s' "$WRTG_DC_IPS_FILE" | tr -d '\r')
	WRTG_NO_CFPROXY=$(printf '%s' "$WRTG_NO_CFPROXY" | tr -d '\r')
	WRTG_NO_WORKER_PASSTHROUGH=$(printf '%s' "$WRTG_NO_WORKER_PASSTHROUGH" | tr -d '\r')
	WRTG_IP_FAIL_COOLDOWN_SEC=$(printf '%s' "$WRTG_IP_FAIL_COOLDOWN_SEC" | tr -d '\r')
	WRTG_WS_POOL_SIZE=$(printf '%s' "$WRTG_WS_POOL_SIZE" | tr -d '\r')
	WRTG_WS_POOL_TTL_SEC=$(printf '%s' "$WRTG_WS_POOL_TTL_SEC" | tr -d '\r')
	WRTG_CF_WORKER_POOL_SIZE=$(printf '%s' "$WRTG_CF_WORKER_POOL_SIZE" | tr -d '\r')
	WRTG_CF_WORKER_POOL_TTL_SEC=$(printf '%s' "$WRTG_CF_WORKER_POOL_TTL_SEC" | tr -d '\r')
	WRTG_WS_BLACKLIST_TTL_SEC=$(printf '%s' "$WRTG_WS_BLACKLIST_TTL_SEC" | tr -d '\r')
	CIDR_URL=$(printf '%s' "$CIDR_URL" | tr -d '\r')
	CIDR_UPDATE_HOUR=$(printf '%s' "$CIDR_UPDATE_HOUR" | tr -d '\r')

	if [ -z "$ROUTER_IP" ]; then
		ROUTER_IP="$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i=1;i<=NF;i++) if ($i=="src") {print $(i+1); exit}}')"
	fi
	[ -n "$ROUTER_IP" ] || ROUTER_IP="127.0.0.1"

	case "$LISTEN" in
		*:*)
			LISTEN_PORT="${LISTEN##*:}"
			;;
		*)
			LISTEN_PORT="$LISTEN"
			;;
	esac
}

default_cidrs() {
	cat <<'EOF'
91.108.4.0/22
91.108.8.0/22
91.108.12.0/22
91.108.16.0/22
91.108.20.0/22
91.108.56.0/22
91.105.192.0/23
149.154.160.0/20
149.154.176.0/20
185.76.151.0/24
EOF
}

load_cidrs() {
	if [ -s "$CIDR_FILE" ]; then
		grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$' "$CIDR_FILE"
	else
		default_cidrs
	fi
}

nft_cidr_elements() {
	load_cidrs | while read -r cidr; do
		[ -n "$cidr" ] || continue
		printf '\t\t\t%s,\n' "$cidr"
	done
}

nft_cidr_inline() {
	# Comma-separated CIDR list for nft add element.
	load_cidrs | while read -r cidr; do
		[ -n "$cidr" ] || continue
		printf '%s, ' "$cidr"
	done | sed 's/, $//'
}

_calls_bypass_delete_rules() {
	chain="$1"
	nft -a list chain inet zapret2 "$chain" 2>/dev/null | \
		grep -F "comment \"$CALLS_NFT_COMMENT\"" | \
		sed -n 's/.*# handle \([0-9]*\).*/\1/p' | \
		sort -rn | while read -r h; do
			[ -n "$h" ] || continue
			nft delete rule inet zapret2 "$chain" handle "$h" 2>/dev/null || true
		done
	# Remove leftover test rules from manual debugging.
	nft -a list chain inet zapret2 "$chain" 2>/dev/null | \
		grep -F 'wrtg-calls-test' | \
		sed -n 's/.*# handle \([0-9]*\).*/\1/p' | \
		sort -rn | while read -r h; do
			[ -n "$h" ] || continue
			nft delete rule inet zapret2 "$chain" handle "$h" 2>/dev/null || true
		done
}

calls_zapret_bypass_remove() {
	nft list table inet zapret2 >/dev/null 2>&1 || return 0
	_calls_bypass_delete_rules postnat_hook
	_calls_bypass_delete_rules prenat_hook
	nft delete set inet zapret2 tg_calls_cidr 2>/dev/null || true
}

calls_zapret_bypass_apply() {
	[ -f "$CALLS_ZAPRET_BYPASS_FLAG" ] || return 0
	nft list table inet zapret2 >/dev/null 2>&1 || {
		echo "zapret2 nft table missing; run after zapret2 start" >&2
		return 1
	}

	calls_zapret_bypass_remove

	ELEMENTS="$(nft_cidr_inline)"
	[ -n "$ELEMENTS" ] || return 1

	nft add set inet zapret2 tg_calls_cidr '{ type ipv4_addr; flags interval; }'
	nft add element inet zapret2 tg_calls_cidr "{ $ELEMENTS }"

	# Outbound call UDP: skip nfqueue (STUN 3478, TURN 596-599, WebRTC ephemerals).
	nft insert rule inet zapret2 postnat_hook position 0 \
		oifname @wanif meta nfproto ipv4 ip daddr @tg_calls_cidr \
		udp dport '{ 3478, 596-599, 50000-65535 }' return \
		comment \"$CALLS_NFT_COMMENT\"

	# Inbound replies from reflectors.
	nft insert rule inet zapret2 prenat_hook position 0 \
		iifname @wanif meta nfproto ipv4 ip saddr @tg_calls_cidr \
		udp sport '{ 3478, 596-599, 50000-65535 }' return \
		comment \"$CALLS_NFT_COMMENT\"

	echo "calls zapret bypass: UDP 3478/596-599/50k+ to telegram CIDR skip nfqueue"
}
