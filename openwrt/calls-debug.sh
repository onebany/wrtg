#!/bin/sh
# Capture UDP traffic relevant to Telegram calls during a test call.
# Run on router while placing/receiving a call from a LAN client.
#
# Usage:
#   sh calls-debug.sh start
#   # ... make a Telegram call ...
#   sh calls-debug.sh stop
#
# Reads telegram CIDR from wrtg if available.

set -e

PCAP=/tmp/tg-calls-debug.pcap
PIDFILE=/tmp/tg-calls-debug.pid
CIDR_FILE="${WRTG_CIDR_FILE:-/var/lib/wrtg/cidrs.txt}"

build_filter() {
	# Reflectors: 91.108.0.0/16 subset in official CIDR; STUN 3478; TURN 596-599; WebRTC ephemerals
	if [ -s "$CIDR_FILE" ]; then
		net=""
		while read -r cidr; do
			case "$cidr" in
				\#*|"") continue ;;
				*/*)
					if [ -n "$net" ]; then
						net="$net or net $cidr"
					else
						net="net $cidr"
					fi
					;;
			esac
		done < "$CIDR_FILE"
		echo "udp and ($net) and (port 3478 or portrange 596-599 or portrange 50000-65535)"
	else
		echo 'udp and (net 91.108.0.0/16 or net 149.154.160.0/20) and (port 3478 or portrange 596-599 or portrange 50000-65535)'
	fi
}

start() {
	if [ -f "$PIDFILE" ] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
		echo "already running pid $(cat "$PIDFILE")"
		exit 1
	fi
	FILTER="$(build_filter)"
	echo "filter: $FILTER"
	tcpdump -i any -nn -s 0 -w "$PCAP" "$FILTER" &
	echo $! > "$PIDFILE"
	echo "capturing to $PCAP (pid $(cat "$PIDFILE"))"
}

stop() {
	if [ ! -f "$PIDFILE" ]; then
		echo "not running"
		exit 1
	fi
	kill "$(cat "$PIDFILE")" 2>/dev/null || true
	rm -f "$PIDFILE"
	echo "stopped; analyze: tcpdump -nn -r $PCAP | head -50"
	tcpdump -nn -r "$PCAP" 2>/dev/null | head -30 || true
}

case "${1:-}" in
	start) start ;;
	stop) stop ;;
	*) echo "usage: $0 {start|stop}"; exit 1 ;;
esac
