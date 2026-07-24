#!/bin/sh
# Shared helpers for wrtg OpenWrt scripts.

CONFIG="${WRTG_CONFIG:-/etc/wrtg/config}"
CIDR_FILE="${WRTG_CIDR_FILE:-/var/lib/wrtg/cidrs.txt}"
# shellcheck disable=SC2034 # consumed by update-cidr.sh after sourcing
CIDR_EXTRA="${WRTG_CIDR_EXTRA:-/etc/wrtg/cidr-extra.txt}"

load_config() {
	ROUTER_IP=""
	LAN_IF=""
	LISTEN="0.0.0.0:8443"
	FRONT_IP="149.154.167.220"
	WRTG_FRONT_DCS=""
	CF_WORKER_DOMAIN=""
	WRTG_CF_WORKER_TOKEN=""
	CF_PROXY_DOMAIN=""
	WRTG_DC_IPS=""
	WRTG_DC_LEARN_FILE=""
	WRTG_DC_IPS_FILE=""
	WRTG_NO_CFPROXY=""
	WRTG_NO_WORKER_PASSTHROUGH=""
	WRTG_CFPROXY_AUTO=""
	WRTG_IP_FAIL_COOLDOWN_SEC=""
	WRTG_FRONTING_SNI=""
	WRTG_FRONTING_COOLDOWN_SEC=""
	WRTG_DC_FAIL_COOLDOWN_SEC=""
	WRTG_WS_FAIL_TIMEOUT_SEC=""
	WRTG_WS_FAIL_TIMEOUT_FAST_SEC=""
	WRTG_WS_POOL_SIZE=""
	WRTG_WS_POOL_TTL_SEC=""
	WRTG_CF_WORKER_POOL_SIZE=""
	WRTG_CF_WORKER_POOL_TTL_SEC=""
	WRTG_WS_BLACKLIST_TTL_SEC=""
	WRTG_CFPROXY_429_COOLDOWN_SEC=""
	WRTG_CFPROXY_429_MAX_COOLDOWN_SEC=""
	WRTG_CFPROXY_PARALLEL=""
	WRTG_DOH_CACHE_SEC=""
	WRTG_WS_PING_SEC=""
	WRTG_TCP_KEEPALIVE_SEC=""
	WRTG_MAX_CONNS=""
	WRTG_SESSION_IDLE_SEC=""
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
	WRTG_CF_WORKER_TOKEN=$(printf '%s' "$WRTG_CF_WORKER_TOKEN" | tr -d '\r')
	CF_PROXY_DOMAIN=$(printf '%s' "$CF_PROXY_DOMAIN" | tr -d '\r')
	WRTG_CFPROXY_AUTO=$(printf '%s' "$WRTG_CFPROXY_AUTO" | tr -d '\r')
	WRTG_DC_IPS=$(printf '%s' "$WRTG_DC_IPS" | tr -d '\r')
	WRTG_DC_LEARN_FILE=$(printf '%s' "$WRTG_DC_LEARN_FILE" | tr -d '\r')
	WRTG_DC_IPS_FILE=$(printf '%s' "$WRTG_DC_IPS_FILE" | tr -d '\r')
	WRTG_NO_CFPROXY=$(printf '%s' "$WRTG_NO_CFPROXY" | tr -d '\r')
	WRTG_NO_WORKER_PASSTHROUGH=$(printf '%s' "$WRTG_NO_WORKER_PASSTHROUGH" | tr -d '\r')
	WRTG_IP_FAIL_COOLDOWN_SEC=$(printf '%s' "$WRTG_IP_FAIL_COOLDOWN_SEC" | tr -d '\r')
	WRTG_FRONTING_SNI=$(printf '%s' "$WRTG_FRONTING_SNI" | tr -d '\r')
	WRTG_FRONTING_COOLDOWN_SEC=$(printf '%s' "$WRTG_FRONTING_COOLDOWN_SEC" | tr -d '\r')
	WRTG_DC_FAIL_COOLDOWN_SEC=$(printf '%s' "$WRTG_DC_FAIL_COOLDOWN_SEC" | tr -d '\r')
	WRTG_WS_FAIL_TIMEOUT_SEC=$(printf '%s' "$WRTG_WS_FAIL_TIMEOUT_SEC" | tr -d '\r')
	WRTG_WS_FAIL_TIMEOUT_FAST_SEC=$(printf '%s' "$WRTG_WS_FAIL_TIMEOUT_FAST_SEC" | tr -d '\r')
	WRTG_WS_POOL_SIZE=$(printf '%s' "$WRTG_WS_POOL_SIZE" | tr -d '\r')
	WRTG_WS_POOL_TTL_SEC=$(printf '%s' "$WRTG_WS_POOL_TTL_SEC" | tr -d '\r')
	WRTG_CF_WORKER_POOL_SIZE=$(printf '%s' "$WRTG_CF_WORKER_POOL_SIZE" | tr -d '\r')
	WRTG_CF_WORKER_POOL_TTL_SEC=$(printf '%s' "$WRTG_CF_WORKER_POOL_TTL_SEC" | tr -d '\r')
	WRTG_WS_BLACKLIST_TTL_SEC=$(printf '%s' "$WRTG_WS_BLACKLIST_TTL_SEC" | tr -d '\r')
	WRTG_CFPROXY_429_COOLDOWN_SEC=$(printf '%s' "$WRTG_CFPROXY_429_COOLDOWN_SEC" | tr -d '\r')
	WRTG_CFPROXY_429_MAX_COOLDOWN_SEC=$(printf '%s' "$WRTG_CFPROXY_429_MAX_COOLDOWN_SEC" | tr -d '\r')
	WRTG_CFPROXY_PARALLEL=$(printf '%s' "$WRTG_CFPROXY_PARALLEL" | tr -d '\r')
	WRTG_DOH_CACHE_SEC=$(printf '%s' "$WRTG_DOH_CACHE_SEC" | tr -d '\r')
	WRTG_WS_PING_SEC=$(printf '%s' "$WRTG_WS_PING_SEC" | tr -d '\r')
	WRTG_TCP_KEEPALIVE_SEC=$(printf '%s' "$WRTG_TCP_KEEPALIVE_SEC" | tr -d '\r')
	WRTG_MAX_CONNS=$(printf '%s' "$WRTG_MAX_CONNS" | tr -d '\r')
	WRTG_SESSION_IDLE_SEC=$(printf '%s' "$WRTG_SESSION_IDLE_SEC" | tr -d '\r')
	CIDR_URL=$(printf '%s' "$CIDR_URL" | tr -d '\r')
	CIDR_UPDATE_HOUR=$(printf '%s' "$CIDR_UPDATE_HOUR" | tr -d '\r')

	if [ -z "$LAN_IF" ]; then
		LAN_IF="$(uci -q get network.lan.device 2>/dev/null || true)"
		[ -n "$LAN_IF" ] || LAN_IF="$(uci -q get network.lan.ifname 2>/dev/null | awk '{print $1}')"
		[ -n "$LAN_IF" ] || {
			ip link show br-lan >/dev/null 2>&1 && LAN_IF="br-lan" || LAN_IF="eth0"
		}
	fi

	# LAN_IF may be a space-separated list; each name goes into an nft iifname
	# expression, so validate strictly (kernel name limit IFNAMSIZ-1 = 15).
	_lan_if_ok=1
	for _if in $LAN_IF; do
		case "$_if" in
			''|*[!A-Za-z0-9._-]*) _lan_if_ok=0 ;;
		esac
		[ "${#_if}" -le 15 ] || _lan_if_ok=0
	done
	[ "$_lan_if_ok" = "1" ] || {
		echo "wrtg: invalid LAN_IF (interface names: letters, digits, -_.; max 15 chars): $LAN_IF" >&2
		return 1
	}

	if [ -z "$ROUTER_IP" ]; then
		ROUTER_IP="$(
			ip -4 addr show dev "$LAN_IF" 2>/dev/null |
				awk '/inet / { split($2, a, "/"); print a[1]; exit }'
		)"
	fi
	[ -n "$ROUTER_IP" ] || {
		echo "wrtg: cannot determine LAN IPv4 for $LAN_IF; set ROUTER_IP" >&2
		return 1
	}

	case "$LISTEN" in
		*:*)
			LISTEN_PORT="${LISTEN##*:}"
			;;
		*)
			LISTEN_PORT="$LISTEN"
			;;
	esac
	case "$LISTEN_PORT" in
		''|*[!0-9]*)
			echo "wrtg: invalid LISTEN port: $LISTEN" >&2
			return 1
			;;
	esac
	if ! { [ "$LISTEN_PORT" -ge 1 ] 2>/dev/null && [ "$LISTEN_PORT" -le 65535 ]; }; then
		echo "wrtg: LISTEN port out of range: $LISTEN_PORT" >&2
		return 1
	fi
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

valid_ipv4_cidrs() {
	awk -F'[./]' '
		NF == 5 {
			for (i = 1; i <= 4; i++) {
				if ($i !~ /^[0-9]+$/ || $i < 0 || $i > 255) next
			}
			if ($5 !~ /^[0-9]+$/ || $5 < 0 || $5 > 32) next
			print $0
		}
	'
}

load_cidrs() {
	if [ -s "$CIDR_FILE" ]; then
		_valid_cidrs="$(valid_ipv4_cidrs < "$CIDR_FILE")"
		if [ -n "$_valid_cidrs" ]; then
			printf '%s\n' "$_valid_cidrs"
		else
			default_cidrs
		fi
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

