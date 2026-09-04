#!/bin/sh
# Sync the wrtg cron lines with /etc/wrtg/config: the daily CIDR refresh and,
# unless WRTG_AUTO_UPDATE="0", the daily unattended update. Idempotent: only
# the two wrtg lines are rewritten, anything else in root's crontab stays.
# Called by install.sh and by the init script on start and reload, so a
# time saved in LuCI lands with "Save & Reload".
set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=lib.sh
. "$ROOT/lib.sh"

load_config

CRON_FILE="${WRTG_CRON_FILE:-/etc/crontabs/root}"
mkdir -p "$(dirname "$CRON_FILE")"
touch "$CRON_FILE"

hour="${CIDR_UPDATE_HOUR:-4}"
case "$hour" in
	[0-9] | 1[0-9] | 2[0-3]) ;;
	*)
		echo "wrtg: CIDR_UPDATE_HOUR='$hour' is not 0-23; using 4" >&2
		hour=4
		;;
esac

# HH:MM -> "M H"; anything else falls back to 06:00 rather than silently
# leaving the job unscheduled.
at="${WRTG_AUTO_UPDATE_TIME:-06:00}"
case "$at" in
	[0-9]:[0-5][0-9] | [01][0-9]:[0-5][0-9] | 2[0-3]:[0-5][0-9]) ;;
	*)
		echo "wrtg: WRTG_AUTO_UPDATE_TIME='$at' is not HH:MM; using 06:00" >&2
		at="06:00"
		;;
esac
at_h="${at%%:*}"
at_m="${at#*:}"
# Strip a leading zero: cron takes "6" and "06" alike, but be tidy.
at_h="${at_h#0}"
at_m="${at_m#0}"
[ -n "$at_h" ] || at_h=0
[ -n "$at_m" ] || at_m=0

tmp="$CRON_FILE.wrtg.$$"
grep -v -e "$ROOT/update-cidr.sh" -e "$ROOT/auto-update.sh" "$CRON_FILE" > "$tmp" || true
echo "0 $hour * * * $ROOT/update-cidr.sh >/dev/null 2>&1" >> "$tmp"
if [ "${WRTG_AUTO_UPDATE:-1}" != "0" ]; then
	echo "$at_m $at_h * * * $ROOT/auto-update.sh >/dev/null 2>&1" >> "$tmp"
fi
mv "$tmp" "$CRON_FILE"

# BusyBox crond only notices an edited crontab on its own schedule; a restart
# makes the new line effective now.
if [ -x /etc/init.d/cron ]; then
	/etc/init.d/cron enable >/dev/null 2>&1 || true
	/etc/init.d/cron restart >/dev/null 2>&1 || true
fi

if [ "${WRTG_AUTO_UPDATE:-1}" != "0" ]; then
	echo "wrtg cron: CIDR refresh at $hour:00, auto-update at $at"
else
	echo "wrtg cron: CIDR refresh at $hour:00, auto-update off"
fi
