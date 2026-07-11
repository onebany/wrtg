#!/bin/sh
# Verify the CF Worker's hardcoded TELEGRAM_CIDRS stays in sync with the
# router-side default_cidrs() in lib.sh. The router refreshes its live CIDR list
# daily, but the Worker's copy is a static snapshot that only changes on a manual
# redeploy — so drift here means the Worker would 403 traffic the router still
# forwards. Run in CI to catch that at release time.
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"

# Extract `["1.2.3.0", 22],` entries from the Worker as `1.2.3.0/22`.
worker_cidrs="$(sed -n 's/.*\["\([0-9.]*\)", *\([0-9]*\)\].*/\1\/\2/p' "$ROOT/cf-worker.js" | sort)"

# shellcheck source=lib.sh
. "$ROOT/lib.sh"
lib_cidrs="$(default_cidrs | sort)"

if [ "$worker_cidrs" != "$lib_cidrs" ]; then
	echo "CIDR drift between cf-worker.js and lib.sh default_cidrs():" >&2
	echo "-- lib.sh default_cidrs() --" >&2
	printf '%s\n' "$lib_cidrs" >&2
	echo "-- cf-worker.js TELEGRAM_CIDRS --" >&2
	printf '%s\n' "$worker_cidrs" >&2
	echo "Update openwrt/cf-worker.js (and redeploy the Worker) to match." >&2
	exit 1
fi

echo "cf-worker.js CIDRs in sync with lib.sh default_cidrs() ($(printf '%s\n' "$worker_cidrs" | grep -c .) entries)"
