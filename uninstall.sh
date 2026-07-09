#!/bin/sh
# Remove wrtg from OpenWrt.
#
#   sh uninstall.sh
#   FORCE=1 sh uninstall.sh    # skip confirmation

set -e

ETC="/etc/wrtg"
INITD="/etc/init.d/wrtg"
FORCE="${FORCE:-0}"

if [ "$FORCE" != "1" ]; then
	printf 'Remove wrtg? [y/N] '
	read -r ans
	case "$ans" in
		y|Y|yes|YES) ;;
		*) echo "Aborted."; exit 0 ;;
	esac
fi

[ -x "$INITD" ] && "$INITD" stop 2>/dev/null || true
[ -x "$INITD" ] && "$INITD" disable 2>/dev/null || true

nft delete table inet tg_tproxy 2>/dev/null || true

CRON_FILE="/etc/crontabs/root"
if [ -f "$CRON_FILE" ]; then
	sed -i '/wrtg\/update-cidr\.sh/d' "$CRON_FILE" 2>/dev/null || \
		grep -v 'wrtg/update-cidr.sh' "$CRON_FILE" > "${CRON_FILE}.tmp" && \
		mv "${CRON_FILE}.tmp" "$CRON_FILE"
fi

rm -f /usr/sbin/wrtg "$INITD"
rm -rf "$ETC"
rm -f /etc/nftables.d/wrtg.nft
rm -rf /var/lib/wrtg
rm -rf /usr/share/ucode/luci/template/wrtg
rm -f /usr/share/luci/menu.d/luci-app-wrtg.json
rm -f /usr/share/rpcd/acl.d/luci-app-wrtg.json
rm -rf /tmp/luci-* /tmp/luci-indexcache 2>/dev/null || true
/etc/init.d/rpcd restart 2>/dev/null || true
/etc/init.d/uhttpd restart 2>/dev/null || true

echo "wrtg uninstalled."
