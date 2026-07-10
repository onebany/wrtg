#!/bin/sh
# wrtg installer for OpenWrt — daemon + LuCI app.
#
#   On the router (from a cloned repo):   sh install.sh
#   From a PC (build + upload via SSH):   ROUTER=root@192.168.1.1 sh install.sh
#
# For a zero-clone install on the router, use the one-liner in the README
# (bootstrap.sh downloads a release bundle, then runs this script).
#
# Options (env):
#   ROUTER=root@host   Remote install over SSH/SCP (builds locally, uploads)
#   SKIP_BUILD=1       Use an existing dist/wrtg-linux-* binary, don't build
#   SKIP_LUCI=1        Don't install the LuCI web app
#   LUCI_ONLY=1        Install only the LuCI app (also: --luci-only)
#   NO_START=1         Install files but don't enable/start the service
#   ASSUME_YES=1       Non-interactive: accept defaults, no prompts (also: -y)
#   FRONT_IP=, LAN_IF=, CF_WORKER_DOMAIN=   Pre-seed config values

set -e

ROOT="$(cd "$(dirname "$0")" && pwd)"
VERSION="$(cat "$ROOT/VERSION" 2>/dev/null || echo dev)"
ROUTER="${ROUTER:-}"
SKIP_BUILD="${SKIP_BUILD:-0}"
SKIP_LUCI="${SKIP_LUCI:-0}"
LUCI_ONLY="${LUCI_ONLY:-0}"
NO_START="${NO_START:-0}"
ASSUME_YES="${ASSUME_YES:-0}"

for arg in "$@"; do
	case "$arg" in
		--luci-only) LUCI_ONLY=1 ;;
		-y|--yes) ASSUME_YES=1 ;;
	esac
done
[ "$LUCI_ONLY" = "1" ] && SKIP_BUILD=1

# ── pretty output ────────────────────────────────────────────────────────────
if [ -t 1 ] && [ -z "$NO_COLOR" ]; then
	C_B="$(printf '\033[1m')"; C_D="$(printf '\033[2m')"; C_G="$(printf '\033[32m')"
	C_Y="$(printf '\033[33m')"; C_R="$(printf '\033[31m')"; C_C="$(printf '\033[36m')"; C_0="$(printf '\033[0m')"
else
	C_B=; C_D=; C_G=; C_Y=; C_R=; C_C=; C_0=
fi
say()  { printf '%s\n' "$*"; }
step() { printf '%s→%s %s\n' "$C_C" "$C_0" "$*"; }
ok()   { printf '%s✓%s %s\n' "$C_G" "$C_0" "$*"; }
warn() { printf '%s!%s %s\n' "$C_Y" "$C_0" "$*"; }
die()  { printf '%s✗ %s%s\n' "$C_R" "$*" "$C_0" >&2; exit 1; }
banner() {
	printf '%s\n' "${C_B}${C_C}"
	printf '%s\n' "  wrtg — transparent Telegram proxy for OpenWrt"
	printf '%s\n' "  v${VERSION}${C_0}${C_D}   (transparent · no client config)${C_0}"
	printf '\n'
}

PKG_DIR="$ROOT/openwrt"
LUCI_DIR="$ROOT/openwrt/luci-app-wrtg"
DIST_DIR="$ROOT/dist"
ETC="/etc/wrtg"
INITD="/etc/init.d/wrtg"
LUCI_TMPL_DST="/usr/share/ucode/luci/template/wrtg"
LUCI_MENU_DST="/usr/share/luci/menu.d/luci-app-wrtg.json"
LUCI_ACL_DST="/usr/share/rpcd/acl.d/luci-app-wrtg.json"
DOCS_SRC="$ROOT/docs"
DOCS_DST="$ETC/docs"

# Collected config (empty = keep config.default value)
CFG_FRONT_IP="${FRONT_IP:-}"
CFG_LAN_IF="${LAN_IF:-}"
CFG_CF_WORKER="${CF_WORKER_DOMAIN:-}"

detect_arch() {
	case "$(uname -m)" in
		x86_64|amd64) echo amd64 ;;
		aarch64|arm64) echo arm64 ;;
		armv7l|armv7|armv6l) echo arm ;;
		*) return 1 ;;
	esac
}

# ── interactive config (only with a TTY, fresh install) ──────────────────────
ask() { # prompt default -> echoes answer
	_p="$1"; _d="$2"; _a=
	if [ "$ASSUME_YES" = "1" ] || [ ! -t 0 ]; then echo "$_d"; return; fi
	printf '%s%s%s [%s]: ' "$C_B" "$_p" "$C_0" "${_d:-none}" >&2
	read -r _a || _a=
	[ -n "$_a" ] && echo "$_a" || echo "$_d"
}

interactive_config() {
	[ "$ASSUME_YES" = "1" ] && return
	[ -t 0 ] || return
	# Only prompt on a fresh install (no existing /etc/wrtg/config)
	if [ -n "$ROUTER" ]; then
		ssh "$ROUTER" '[ -f /etc/wrtg/config ]' 2>/dev/null && return 0
	else
		[ -f "$ETC/config" ] && return 0
	fi
	say ""
	say "${C_B}Setup${C_0} ${C_D}(press Enter to accept defaults)${C_0}"
	[ -z "$CFG_LAN_IF" ] && CFG_LAN_IF="$(ask 'LAN interface (clients side, empty=auto)' '')"
	[ -z "$CFG_FRONT_IP" ] && CFG_FRONT_IP="$(ask 'Front IP (Telegram entry)' '149.154.167.220')"
	if [ -z "$CFG_CF_WORKER" ]; then
		say "${C_D}Cloudflare Worker — fixes DC1/3/5, stickers & animated emoji.${C_0}"
		say "${C_D}Leave empty to set later (LuCI -> Settings, or ${ETC}/config). Guide: docs/GUIDE.md${C_0}"
		CFG_CF_WORKER="$(ask 'CF_WORKER_DOMAIN (optional)' '')"
	fi
	say ""
}

# render config.default with collected overrides -> stdout
render_config() {
	awk -v fip="$CFG_FRONT_IP" -v lif="$CFG_LAN_IF" -v cfw="$CFG_CF_WORKER" '
		/^FRONT_IP=/   && fip != "" { print "FRONT_IP=\"" fip "\""; next }
		/^LAN_IF=/     && lif != "" { print "LAN_IF=\"" lif "\""; next }
		/^# *CF_WORKER_DOMAIN=/ && cfw != "" { print "CF_WORKER_DOMAIN=\"" cfw "\""; next }
		{ print }
	' "$PKG_DIR/config.default"
}

build_binary() {
	command -v cargo >/dev/null 2>&1 || command -v rustup >/dev/null 2>&1 || \
		die "Rust toolchain not found. Install it (https://rustup.rs) or use a release binary (SKIP_BUILD=1)."
	step "Building wrtg $VERSION for linux/$1 (musl static)..."
	sh "$ROOT/build-rust.sh" "$1" >/dev/null
	ok "Built dist/wrtg-linux-$1"
}

pick_binary() {
	arch="$(detect_arch)" || die "Unsupported CPU: $(uname -m)"
	bin="$DIST_DIR/wrtg-linux-$arch"
	if [ -x "$bin" ] || { [ "$SKIP_BUILD" = "1" ] && [ -f "$bin" ]; }; then
		echo "$bin"
		return
	fi
	[ "$SKIP_BUILD" = "1" ] && die "Binary not found: $bin (SKIP_BUILD=1)"
	build_binary "$arch" >&2
	echo "$bin"
}

check_deps() { # runs on the target (local install)
	command -v nft >/dev/null 2>&1 || warn "nftables 'nft' not found - install: opkg update && opkg install nftables kmod-nft-nat"
	[ -d /etc/init.d ] || warn "This does not look like OpenWrt."
}

# ── LuCI ─────────────────────────────────────────────────────────────────────
LUCI_FILES="status.ut config.ut logs.ut action.ut docs.ut"
DOC_FILES="GUIDE.md"

install_luci_local() {
	[ "$SKIP_LUCI" = "1" ] && return
	step "Installing LuCI web app..."
	mkdir -p "$LUCI_TMPL_DST" "$(dirname "$LUCI_MENU_DST")" "$(dirname "$LUCI_ACL_DST")" "$DOCS_DST"
	install -m 644 "$LUCI_DIR/root/usr/share/ucode/luci/template/wrtg/"*.ut "$LUCI_TMPL_DST/"
	install -m 644 "$LUCI_DIR/root/usr/share/luci/menu.d/luci-app-wrtg.json" "$LUCI_MENU_DST"
	install -m 644 "$LUCI_DIR/root/usr/share/rpcd/acl.d/luci-app-wrtg.json" "$LUCI_ACL_DST"
	for f in ARCHITECTURE.md DEVELOPMENT.md CF_WORKER_SETUP.md CF_PROXY.md; do rm -f "$DOCS_DST/$f"; done
	for f in $DOC_FILES; do [ -f "$DOCS_SRC/$f" ] && install -m 644 "$DOCS_SRC/$f" "$DOCS_DST/$f"; done
	install -m 644 "$ROOT/VERSION" "$ETC/version"
	rm -f /usr/lib/lua/luci/controller/wrtg.lua /usr/lib/lua/luci/model/cbi/wrtg.lua 2>/dev/null || true
	rm -rf /usr/lib/lua/luci/view/wrtg /tmp/luci-* /tmp/luci-indexcache 2>/dev/null || true
	/etc/init.d/rpcd restart 2>/dev/null || true
	/etc/init.d/uhttpd restart 2>/dev/null || true
	ok "LuCI installed (Services -> wrtg)"
}

# ── daemon (local) ───────────────────────────────────────────────────────────
install_files() {
	step "Installing daemon + service files..."
	mkdir -p "$ETC" /usr/sbin /var/lib/wrtg
	# mv-into-place: overwriting a running binary directly fails with ETXTBSY.
	install -m 755 "$1" /usr/sbin/wrtg.new && mv /usr/sbin/wrtg.new /usr/sbin/wrtg
	for f in lib.sh setup-nft.sh update-cidr.sh; do
		install -m 755 "$PKG_DIR/$f" "$ETC/$f"
	done
	install -m 644 "$PKG_DIR/cidr-extra.txt" "$ETC/cidr-extra.txt"
	install -m 644 "$PKG_DIR/cf-worker.js" "$ETC/cf-worker.js"
	install -m 755 "$PKG_DIR/wrtg.init" "$INITD"
	install -m 644 "$ROOT/VERSION" "$ETC/version"
	rm -f "$ETC/zapret-telegram-calls.sh" "$ETC/calls-debug.sh" /etc/nftables.d/wrtg.nft

	# IP→DC maps: ship template if missing; always ensure learned file exists.
	if [ ! -f "$ETC/dc-ips.txt" ] && [ -f "$PKG_DIR/dc-ips.txt" ]; then
		install -m 644 "$PKG_DIR/dc-ips.txt" "$ETC/dc-ips.txt"
		ok "Wrote $ETC/dc-ips.txt"
	fi
	[ -f "$ETC/dc-ips-learned.txt" ] || touch "$ETC/dc-ips-learned.txt"

	if [ ! -f "$ETC/config" ]; then
		render_config > "$ETC/config"
		chmod 600 "$ETC/config"
		ok "Wrote $ETC/config"
	else
		chmod 600 "$ETC/config"
		ok "Kept existing $ETC/config"
	fi

	# shellcheck disable=SC1090
	. "$ETC/lib.sh"; load_config
	CRON_FILE="/etc/crontabs/root"; mkdir -p "$(dirname "$CRON_FILE")"; touch "$CRON_FILE"
	grep -qF "$ETC/update-cidr.sh" "$CRON_FILE" 2>/dev/null || \
		echo "0 ${CIDR_UPDATE_HOUR:-4} * * * $ETC/update-cidr.sh >/dev/null 2>&1" >> "$CRON_FILE"

	step "Fetching Telegram CIDR + loading nftables..."
	"$ETC/update-cidr.sh" >/dev/null 2>&1 || warn "CIDR fetch failed; using built-in defaults"

	if [ "$NO_START" != "1" ]; then
		"$INITD" enable
		"$INITD" restart
		[ -x /etc/init.d/cron ] && { /etc/init.d/cron enable 2>/dev/null || true; /etc/init.d/cron start 2>/dev/null || true; }
	fi
}

verify_local() {
	sleep 1
	if pidof wrtg >/dev/null 2>&1; then ok "Service running (PID $(pidof wrtg))"; else warn "Service not running - check: logread -e wrtg"; fi
	if nft list table inet tg_tproxy >/dev/null 2>&1; then ok "nftables DNAT loaded"; else warn "nft table missing - check LAN_IF in $ETC/config"; fi
}

summary() {
	CFW="$(sed -n 's/^CF_WORKER_DOMAIN="\(.*\)"/\1/p' "$ETC/config" 2>/dev/null)"
	say ""
	say "${C_G}${C_B}wrtg $VERSION installed.${C_0}"
	say "  ${C_D}Config${C_0}  $ETC/config"
	say "  ${C_D}Status${C_0}  $INITD status   ${C_D}.  Logs${C_0}  logread -e wrtg"
	[ "$SKIP_LUCI" != "1" ] && say "  ${C_D}Web${C_0}     Services -> wrtg  (/cgi-bin/luci/admin/services/wrtg/status)"
	say ""
	say "  Open Telegram on a LAN device - logs should show ${C_C}direct handshake OK${C_0} / ${C_C}WS connected${C_0}."
	if [ -z "$CFW" ]; then
		warn "No CF Worker set - DC1/3/5, stickers and animated emoji need one."
		say "     ${C_D}5-min setup: docs/GUIDE.md -> then set CF_WORKER_DOMAIN and restart.${C_0}"
	fi
	say ""
}

install_local() {
	check_deps
	if [ "$LUCI_ONLY" != "1" ]; then
		BIN="$(pick_binary)"
		install_files "$BIN"
	fi
	install_luci_local
	[ "$LUCI_ONLY" != "1" ] && [ "$NO_START" != "1" ] && verify_local
	summary
}

# ── remote (from a PC) ───────────────────────────────────────────────────────
install_remote() {
	command -v ssh >/dev/null 2>&1 || die "ssh not found on this PC."
	step "Detecting router architecture..."
	rarch="$(ssh -o StrictHostKeyChecking=accept-new "$ROUTER" 'uname -m' | tr -d '\r')" || die "Cannot reach $ROUTER over SSH."
	case "$rarch" in
		x86_64|amd64) garch=amd64 ;; aarch64|arm64) garch=arm64 ;; armv7l|armv7|armv6l) garch=arm ;;
		*) die "Unsupported router arch: $rarch" ;;
	esac
	ok "Router: $rarch -> wrtg-linux-$garch"

	if [ "$LUCI_ONLY" != "1" ]; then
		BIN="$DIST_DIR/wrtg-linux-$garch"
		if [ -x "$BIN" ] || { [ "$SKIP_BUILD" = "1" ] && [ -f "$BIN" ]; }; then
			:
		elif [ "$SKIP_BUILD" = "1" ]; then
			die "Binary not found: $BIN"
		else
			build_binary "$garch"
		fi
		step "Uploading daemon to $ROUTER..."
		ssh "$ROUTER" "mkdir -p $ETC /var/lib/wrtg"
		scp -qO "$BIN" "$ROUTER:/usr/sbin/wrtg.new"
		for f in lib.sh setup-nft.sh update-cidr.sh cidr-extra.txt cf-worker.js; do
			scp -qO "$PKG_DIR/$f" "$ROUTER:$ETC/$f"
		done
		# Seed dc-ips.txt only when missing (don't clobber admin edits).
		scp -qO "$PKG_DIR/dc-ips.txt" "$ROUTER:$ETC/dc-ips.txt.shipped"
		scp -qO "$PKG_DIR/wrtg.init" "$ROUTER:$INITD"
		scp -qO "$PKG_DIR/config.default" "$ROUTER:$ETC/config.default"
		render_config | ssh "$ROUTER" "cat > $ETC/config.new"
		scp -qO "$ROOT/VERSION" "$ROUTER:$ETC/version"
		ok "Uploaded"

		step "Configuring service on router..."
		ssh "$ROUTER" "NO_START=$NO_START sh -s" <<'REMOTE'
set -e
ETC=/etc/wrtg; INITD=/etc/init.d/wrtg
# mv-into-place: overwriting the running binary directly fails with ETXTBSY.
[ -f /usr/sbin/wrtg.new ] && mv /usr/sbin/wrtg.new /usr/sbin/wrtg
chmod +x /usr/sbin/wrtg "$ETC"/*.sh "$INITD"
rm -f "$ETC/zapret-telegram-calls.sh" "$ETC/calls-debug.sh" /etc/nftables.d/wrtg.nft
if [ -f "$ETC/config" ]; then rm -f "$ETC/config.new"; else mv "$ETC/config.new" "$ETC/config"; fi
chmod 600 "$ETC/config"
if [ ! -f "$ETC/dc-ips.txt" ] && [ -f "$ETC/dc-ips.txt.shipped" ]; then
	mv "$ETC/dc-ips.txt.shipped" "$ETC/dc-ips.txt"
else
	rm -f "$ETC/dc-ips.txt.shipped"
fi
[ -f "$ETC/dc-ips-learned.txt" ] || touch "$ETC/dc-ips-learned.txt"
. "$ETC/lib.sh"; load_config
CRON=/etc/crontabs/root; mkdir -p "$(dirname "$CRON")"; touch "$CRON"
grep -qF "$ETC/update-cidr.sh" "$CRON" 2>/dev/null || echo "0 ${CIDR_UPDATE_HOUR:-4} * * * $ETC/update-cidr.sh >/dev/null 2>&1" >> "$CRON"
"$ETC/update-cidr.sh" >/dev/null 2>&1 || true
if [ "$NO_START" != "1" ]; then
	"$INITD" enable; "$INITD" restart
	[ -x /etc/init.d/cron ] && { /etc/init.d/cron enable 2>/dev/null || true; /etc/init.d/cron start 2>/dev/null || true; }
fi
sleep 1
pidof wrtg >/dev/null 2>&1 && echo "  service running (PID $(pidof wrtg))" || echo "  ! service not running"
nft list table inet tg_tproxy >/dev/null 2>&1 && echo "  nftables DNAT loaded" || echo "  ! nft table missing"
REMOTE
		ok "Daemon configured"
	fi

	if [ "$SKIP_LUCI" != "1" ]; then
		step "Uploading LuCI web app..."
		ssh "$ROUTER" "mkdir -p $LUCI_TMPL_DST $(dirname "$LUCI_MENU_DST") $(dirname "$LUCI_ACL_DST") $DOCS_DST; rm -f $DOCS_DST/ARCHITECTURE.md $DOCS_DST/DEVELOPMENT.md $DOCS_DST/CF_WORKER_SETUP.md $DOCS_DST/CF_PROXY.md"
		for f in $LUCI_FILES; do scp -qO "$LUCI_DIR/root/usr/share/ucode/luci/template/wrtg/$f" "$ROUTER:$LUCI_TMPL_DST/"; done
		scp -qO "$LUCI_DIR/root/usr/share/luci/menu.d/luci-app-wrtg.json" "$ROUTER:$LUCI_MENU_DST"
		scp -qO "$LUCI_DIR/root/usr/share/rpcd/acl.d/luci-app-wrtg.json" "$ROUTER:$LUCI_ACL_DST"
		for f in $DOC_FILES; do [ -f "$DOCS_SRC/$f" ] && scp -qO "$DOCS_SRC/$f" "$ROUTER:$DOCS_DST/"; done
		ssh "$ROUTER" "rm -f /usr/lib/lua/luci/controller/wrtg.lua /usr/lib/lua/luci/model/cbi/wrtg.lua; rm -rf /usr/lib/lua/luci/view/wrtg /tmp/luci-* /tmp/luci-indexcache 2>/dev/null; /etc/init.d/rpcd restart; /etc/init.d/uhttpd restart" 2>/dev/null || true
		ok "LuCI installed (Services -> wrtg)"
	fi

	say ""
	say "${C_G}${C_B}wrtg $VERSION installed on $ROUTER.${C_0}"
	say "  ${C_D}Status${C_0}  ssh $ROUTER $INITD status   ${C_D}.  Logs${C_0}  ssh $ROUTER logread -e wrtg"
	[ "$SKIP_LUCI" != "1" ] && say "  ${C_D}Web${C_0}     http://<router>/cgi-bin/luci -> Services -> wrtg"
	say ""
}

banner
interactive_config
if [ -n "$ROUTER" ]; then
	install_remote
else
	install_local
fi
