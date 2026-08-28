#!/bin/sh
# Compare two wrtg builds on a live router by the counters that matter.
#
# Why this exists: twice in one evening a change shipped on green unit tests
# and a one-minute counter reading, and twice it made a router worse. A minute
# is nothing against a bursty failure mode, and a reading of the new build says
# nothing without the same reading of the one it replaced. This runs both sides
# under the same traffic, in the same window, and prints them next to each other.
#
# Usage:
#   ROUTER=root@192.168.20.1 sh tools/ab-compare.sh A.bin B.bin [seconds]
#
# It leaves the router on the build that was there when it started.

set -eu

ROUTER="${ROUTER:-}"
A="${1:-}"
B="${2:-}"
WINDOW="${3:-180}"
SETTLE="${SETTLE:-45}"

die() { echo "ab-compare: $*" >&2; exit 1; }

[ -n "$ROUTER" ] || die "set ROUTER=root@<ip>"
[ -n "$A" ] && [ -n "$B" ] || die "usage: ROUTER=root@ip sh $0 <build-A> <build-B> [seconds]"
[ -f "$A" ] || die "not a file: $A"
[ -f "$B" ] || die "not a file: $B"

ssh -o BatchMode=yes "$ROUTER" true 2>/dev/null || die "cannot reach $ROUTER over SSH"

# Counters worth comparing. `blind_relay` is deliberately absent: it also counts
# ordinary non-MTProto passthrough, so it moves with traffic rather than health.
KEYS="accepted ws_pool_hit ws_direct cf_worker cf_proxy tcp_fallback all_paths_failed"

remote() { ssh -o BatchMode=yes "$ROUTER" "$@"; }

snapshot() { # -> "key value" lines
	remote 'wrtg --stats 2>/dev/null' | tr -s ' ' | awk '/^ [a-z_]+ [0-9]+$/ { print $1, $2 }'
}

value_of() { # keyfile key
	awk -v k="$2" '$1 == k { print $2 }' "$1"
}

install_build() { # local-binary
	remote 'cat > /tmp/ab-wrtg && chmod +x /tmp/ab-wrtg && cp /tmp/ab-wrtg /usr/sbin/wrtg && /etc/init.d/wrtg restart >/dev/null 2>&1' < "$1"
	sleep 8
}

measure() { # label local-binary outfile
	echo "→ $1: installing and settling ${SETTLE}s ..." >&2
	install_build "$2"
	sleep "$SETTLE"
	snapshot > /tmp/ab-before.$$
	echo "→ $1: sampling ${WINDOW}s ..." >&2
	sleep "$WINDOW"
	snapshot > /tmp/ab-after.$$
	: > "$3"
	for k in $KEYS; do
		b=$(value_of /tmp/ab-before.$$ "$k")
		a=$(value_of /tmp/ab-after.$$ "$k")
		[ -n "$b" ] && [ -n "$a" ] || continue
		echo "$k $((a - b))" >> "$3"
	done
	rm -f /tmp/ab-before.$$ /tmp/ab-after.$$
}

echo "→ saving the build currently on $ROUTER" >&2
remote 'cp /usr/sbin/wrtg /tmp/ab-original' || die "cannot snapshot the running build"
restore() {
	echo "→ restoring the original build" >&2
	remote 'cp /tmp/ab-original /usr/sbin/wrtg && /etc/init.d/wrtg restart >/dev/null 2>&1 && rm -f /tmp/ab-original /tmp/ab-wrtg' || true
}
trap restore EXIT INT TERM

measure "A ($(basename "$A"))" "$A" /tmp/ab-a.$$
measure "B ($(basename "$B"))" "$B" /tmp/ab-b.$$

printf '\n%-20s %12s %12s   %s\n' "counter" "A" "B" "verdict"
printf '%s\n' "----------------------------------------------------------------"
verdict_lines=""
for k in $KEYS; do
	a=$(value_of /tmp/ab-a.$$ "$k"); b=$(value_of /tmp/ab-b.$$ "$k")
	[ -n "$a" ] && [ -n "$b" ] || continue
	note=""
	# Only failures get a verdict. Everything else moves with the traffic the
	# router happened to see and comparing it would invite reading noise.
	if [ "$k" = "all_paths_failed" ]; then
		if [ "$((a + b))" -lt 10 ]; then
			# Both sides healthy. This is not "the builds are equivalent": a
			# fallback bug only shows once the path it degrades is under load,
			# and 0.5.38 read clean here on the very router it had broken hours
			# earlier, once the CF Proxy pool recovered. Come back when the
			# counter is moving, or the comparison proves nothing.
			note="INCONCLUSIVE — too few failures to compare"
		elif [ "$b" -gt "$((a * 2 + 5))" ]; then note="B IS WORSE"
		elif [ "$a" -gt "$((b * 2 + 5))" ]; then note="B is better"
		else note="no clear difference"; fi
		verdict_lines="$note"
	fi
	printf '%-20s %12s %12s   %s\n' "$k" "+$a" "+$b" "$note"
done

acc_a=$(value_of /tmp/ab-a.$$ accepted); acc_b=$(value_of /tmp/ab-b.$$ accepted)
printf '\nTraffic differed by %s%%: read the failure counter, not the totals.\n' \
	"$(awk -v x="$acc_a" -v y="$acc_b" 'BEGIN { if (x+y == 0) print 0; else printf "%.0f", 100*(y-x)/((x+y)/2) }')"
echo "Verdict on all_paths_failed: ${verdict_lines:-unavailable}"
case "$verdict_lines" in
	INCONCLUSIVE*)
		echo
		echo "Nothing failed on either build during this run, so it cannot tell them"
		echo "apart. Fallback regressions surface when the rung they touch is under"
		echo "load; run this again while all_paths_failed is actually moving."
		;;
esac
rm -f /tmp/ab-a.$$ /tmp/ab-b.$$
