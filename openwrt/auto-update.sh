#!/bin/sh
# Daily unattended update, run from cron (see setup-cron.sh): check GitHub for
# a newer release and install it with the same path the LuCI Update button
# uses. The outcome goes to $LAST for the status and settings pages and to
# syslog as "wrtg: auto-update: ...".
#
# The whole script sits in one function called on the last line: install.sh
# replaces this very file mid-run, and a shell that is still reading it would
# otherwise execute a mix of old and new text.

ROOT="$(cd "$(dirname "$0")" && pwd)"
LAST="${WRTG_AUTO_UPDATE_LAST:-/etc/wrtg/auto-update.last}"
STATUS_CACHE=/tmp/wrtg-update-status
LOG=/tmp/wrtg-auto-update.log

main() {
	# shellcheck source=lib.sh
	. "$ROOT/lib.sh"
	load_config
	[ "${WRTG_AUTO_UPDATE:-1}" != "0" ] || exit 0

	note() { # result detail
		logger -t wrtg "auto-update: $1: $2" 2>/dev/null || true
		printf 'DATE=%s\nRESULT=%s\nDETAIL=%s\n' "$(date '+%Y-%m-%d %H:%M')" "$1" "$2" > "$LAST" 2>/dev/null || true
	}
	reason() { # stdin: script output -> its last "wrtg:" line, else its last line
		_in="$(cat)"
		_r="$(printf '%s\n' "$_in" | grep '^wrtg: ' | tail -n1 | sed 's/^wrtg: //')"
		if [ -n "$_r" ]; then
			printf '%s' "$_r"
		else
			printf '%s\n' "$_in" | tail -n1
		fi
	}

	out="$(sh "$ROOT/check-update.sh" check 2>&1)" || {
		note failed "check: $(printf '%s\n' "$out" | reason)"
		exit 1
	}
	# Same file the LuCI "Check for updates" button writes, so the status page
	# shows the nightly result without a manual check.
	printf '%s\n' "$out" | grep -E '^(CURRENT|LATEST|AVAILABLE|STATUS)=' > "$STATUS_CACHE" 2>/dev/null || true

	current="$(printf '%s\n' "$out" | sed -n 's/^CURRENT=//p')"
	latest="$(printf '%s\n' "$out" | sed -n 's/^LATEST=//p')"
	if ! printf '%s\n' "$out" | grep -qx 'AVAILABLE=1'; then
		note ok "up to date ($current, latest $latest)"
		exit 0
	fi

	logger -t wrtg "auto-update: installing $latest over $current" 2>/dev/null || true
	if sh "$ROOT/check-update.sh" update > "$LOG" 2>&1; then
		note updated "$current -> $latest"
	else
		note failed "update to $latest: $(reason < "$LOG")"
		exit 1
	fi
}

main "$@"; exit $?
