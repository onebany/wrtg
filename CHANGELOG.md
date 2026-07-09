# Changelog

## 0.4.4 — 2026-07-09

### Added
- **Self-learning IP → DC map (`dc_learn`)** — the compiled-in IP→DC tables only
  covered a few datacenter IPs, so a fresh/rotated Telegram IP whose DC wasn't
  embedded in the handshake (e.g. Telegram for Android) fell to a slow blind
  passthrough via the CF worker. Now, connections that **do** embed a valid DC
  teach `orig_ip → (dc, media)`; connections that don't are resolved from what
  was learned, routing them via the fast front instead. Learned entries persist
  to `/etc/wrtg/dc-ips-learned.txt` (append-only, flash-friendly) and survive
  restarts; an admin-editable `/etc/wrtg/dc-ips.txt` is also loaded at startup.
  Paths override via `WRTG_DC_LEARN_FILE` / `WRTG_DC_IPS_FILE`.
- **DC2 endpoint `149.154.167.35`** added to the curated alt-IP table (seen on
  Telegram for Android / Pixel; previously fell to blind passthrough).
- **LuCI status: dc_learn** — Status page shows learned mapping count and a
  preview of `/etc/wrtg/dc-ips-learned.txt`, with a note about the admin file
  `/etc/wrtg/dc-ips.txt`.
- **`install.sh` deploys `dc-ips.txt`** — ships the template and ensures
  `dc-ips-learned.txt` exists on the router.

### Fixed
- **Worker passthrough for media** — `try_worker_passthrough` only tried the
  **first** CF Worker and logged connect failures at `debug`, so media HTTP/TLS
  silently fell back to `passthrough -> FRONT_IP` with no clue in `logread`.
  Now tries every configured worker, logs attempts at INFO, and failures at
  WARN (skip reasons included).
- **procd env overwrite** — repeated `procd_set_param env KEY=val` calls
  **replace** the previous env list, so only the last variable
  (`WRTG_CF_WORKER_POOL_SIZE`) reached the daemon and `CF_WORKER_DOMAIN` was
  dropped → `cf-workers=0`, no worker passthrough. Fixed with
  `procd_set_param` + `procd_append_param`.

### Removed
- **wrtgo (Go)** — sibling repo deleted 2026-07-07; use wrtg only. Legacy `tg_wrt/legacy/tproxy-go` is superseded.

## 0.4.3 — 2026-07-08

### Added
- **Worker passthrough for media/emoji** — TLS / MTProto-over-HTTP media traffic that can't be MTProto-bridged (so it would `blind_relay` to the front, which returns HTTP 302) now tunnels through the CF Worker to the **real DC IP:port**. Fixes emoji/stickers on transparent-mode networks where only the front is reachable. Requires the Worker to honour the `port` query param (see below); falls back to front passthrough if the Worker is unreachable. Disable with `WRTG_NO_WORKER_PASSTHROUGH=1`.
- **CF Worker `port` param** — `wss://<worker>/apiws?dst=IP&dc=N&port=P` (default 443). Worker code updated in `openwrt/CfWorker.md` / `docs/CF_WORKER_SETUP.md`; backward-compatible (existing MTProto path unaffected).
- **Richer LuCI dashboard** — Status page shows service/routing/CF-worker cards, per-DC last outcome, activity counts and auto-refresh; Logs get filter, colour highlighting and auto-refresh.
- **Guided CF Worker section in LuCI Settings** — a dedicated per-router panel with a configured/not-set badge, a collapsible 5-step "create your Worker" how-to (links to Documentation → CF Worker Setup for the code), the `CF_WORKER_DOMAIN` field and Save & Restart. Plus a quick-set form (FRONT_IP / WRTG_FRONT_DCS / CF_PROXY_DOMAIN) and raw editor.

### Fixed
- **LuCI service buttons** — `action.ut` / config used `import { system } from 'fs'`, but `system()` is a global builtin (not an `fs` export), so start/stop/restart/reload were broken. Now call the global.
- **Worker relay teardown** — the raw passthrough tunnel now tears down as soon as either side closes (select + abort), so a stalled upstream can't leak the connection.

### Removed
- Dead client-side LuCI `.js` views (`htdocs/.../view/wrtg/*.js`) — routing uses the ucode `.ut` templates.
- Redundant dev scripts `deploy-router.sh`, `fix-router-config.sh`, `build-musl-local.sh` (hardcoded IPs / duplicated `build-rust.sh`); superseded by `install.sh`.

### Release prep
- **Friendly `install.sh`** — coloured progress, dependency check, TTY-gated setup prompts (LAN_IF / FRONT_IP / CF_WORKER_DOMAIN with defaults), `mv`-into-place binary swap (no ETXTBSY), post-install verification and a clear summary with next steps.
- **One-line router install** — `bootstrap.sh` downloads a release bundle and runs `install.sh` (no git/Rust). Release workflow now publishes `wrtg-openwrt.tar.gz` (binaries + service files + LuCI + docs) alongside the per-arch binaries.
- **Docs** — README rewritten (quickstart-first, current); CF_WORKER_SETUP covers media/emoji passthrough; ARCHITECTURE shows the worker-passthrough branch.

## 0.4.2 — 2026-07-07

### Added
- **LuCI documentation page** — `docs.ut` with tabs for Architecture, Development, CF Worker Setup, CF Proxy; markdown deployed to `/etc/wrtg/docs/` during install

## 0.4.1 — 2026-07-07

### Added
- **Unified installer** — `install.sh` deploys both the wrtg binary and the LuCI ucode app in one step (`ROUTER=root@IP sh install.sh`). Use `SKIP_LUCI=1` to skip LuCI, `--luci-only` / `install-luci.sh` for LuCI-only
- **LuCI on ucode** — `luci-app-wrtg` rewritten without Lua: ucode templates (`status`, `config`, `logs`, `action`) in `/usr/share/ucode/luci/template/wrtg/`, `menu.d` JSON routing; legacy Lua files removed on install

### Fixed
- LuCI install cleans up old Lua controller/CBI/views if present from prior versions

## 0.4.0 — 2026-07-07

### Added
- **GitHub-IP pinning for CF-proxy list refresh** — `raw.githubusercontent.com` connects to pinned Fastly IPs (`185.199.108-111.133`) first, with system DNS as fallback (TLS SNI/Host unchanged). Keeps the domain-list refresh working when the ISP poisons the hostname's DNS (matches the reference tg-ws-proxy).
- **Configurable front scope (`WRTG_FRONT_DCS`)** — the global `FRONT_IP` now applies only to the DCs it can actually front (default `2,4`); other DCs resolve to their **real IP** for direct WS / correct CF-worker `dst`. This makes wrtg adaptive across networks (direct where reachable, front only where it works) and matches the reference tg-ws-proxy `dc_redirects`. Values: `2,4` (default), `all`, `none`, or an explicit list. Per-DC `DC{N}_FRONT_IP` overrides still win.
  - **Bug fixed by this:** previously `try_cf_fallback` handed the CF Worker `dst=FRONT_IP` for *every* DC, so DC1/3/5 told the Worker to connect to `167.220` (which can't route them) → the Worker failed too. With scoping, DC1/3/5 get their real DC IP as `dst`, so the Worker reaches the actual datacenter.

### Fixed
- **Flaky `ip_fail` test** — the two `ip_fail` tests raced on the global map + `reset_all()`, and `std::env::set_var` raced across threads. Now serialized with a test mutex and `ip_fail_expiry` is deterministic (inserts past/future `Instant`s directly, no env, no sleep). Verified 5×5 green
- **CF Proxy round-robin seed** — `proxy_domains_for_dc` now advances `PROXY_RR` instead of `WORKER_RR`; the shared `ordered_domains` helper takes the counter explicitly
- **Accept-loop watchdog starvation** — dropped `Arc<Mutex<TcpListener>>`; the accept loop no longer holds a lock across `accept().await`. `watchdog::serve` owns the listener and self-heals by rebinding a fresh transparent socket after a run of consecutive `accept()` errors (with backoff), instead of a lock-guarded `local_addr()` poll
- **SIGHUP reverted custom `FRONT_IP`** — `wrtg.init` now also exports `FRONT_IP` as env; previously it was passed only as the startup-only `-front-ip` CLI arg, so `reload` (which re-reads env) reset a customized front IP to the default
- **Unplumbed tunables** — `WRTG_WS_POOL_TTL_SEC`, `WRTG_CF_WORKER_POOL_SIZE`, `WRTG_CF_WORKER_POOL_TTL_SEC` were documented in `config.default` but never passed to the daemon; now wired through `lib.sh` + `wrtg.init`
- **`ws_domains` ignored `is_media`** — always tried `kws{N}` before `kws{N}-1`. Media now tries the `kws{N}-1` CDN host first (matches the reference tg-ws-proxy ordering); non-media unchanged

### Changed
- Cleaned up all `clippy` warnings (dead `all_blocked` assignments, redundant `mut`, `while let` loops, `clamp`, boolean simplification); `cargo clippy --all-targets` is now warning-free
- `config.default` rewritten: grouped (Core / Cloudflare / Tuning / CIDR), each variable marked reload- vs restart-scoped

## 0.3.0 — 2026-07-07

### Added
- **CF Worker fallback** — `connect_cf_worker_ws` wired into bridge chain (`wss://<worker>/apiws?dst=...&dc=...`)
- **cf_worker_pool** — pre-warmed CF Worker connections per DC
- **CF Proxy balancer** — round-robin across `CF_PROXY_DOMAIN` values (`wss://kws{N}.<domain>/apiws`)
- **ip_fail_until** — cooldown on FRONT_IP after WS connect timeouts (`WRTG_IP_FAIL_COOLDOWN_SEC`)
- **Per-DC FRONT_IP** — `DC{N}_FRONT_IP` / `WRTG_DC_IPS=1:ip,2:ip`
- **Media DC1 after blacklist** — improved TCP fallback for media CDN when WS blacklisted
- **Config hot-reload** — SIGHUP reloads env config; `/etc/init.d/wrtg reload`
- **Health watchdog** — internal listener health check and rebind
- **LuCI minimal** — `luci-app-wrtg` (status, FRONT_IP, start/stop/reload, logs)

### Changed
- Fallback chain: skip WS (blacklist/ip_fail) → pool WS → direct WS → CF Worker pool → CF Proxy → TCP → blind relay
- `ws_pool` warmup uses per-DC front IP
- CIDR docs for `91.108.x` reflector subnets

## 0.2.0

- WS connection pool, TTL blacklist, WS split read/write, TCP fallback, blind relay
