# Changelog

## 0.5.25 - 2026-07-20

### Fixed
- **`JoinHandle polled after completion` panics** — after a clean drain in `bridge_ws`/`bridge_tcp` the surviving task's JoinHandle was awaited a second time, panicking the connection task at the end of nearly every session. The handle is now only awaited on the abort path.
- **Direct-WS flap loop under sustained ISP WS blocks** — a successful fallback (CF Worker/CF Proxy/TCP) cleared the `ip_fail`/`dc_fail`/WS-blacklist state, so the very next connection retried direct WS, paid the full connect timeout again and re-marked it. Observed as Telegram media stalling several seconds per new connection when the ISP starts dropping direct WS to Telegram IPs. Fallback success no longer touches direct-path skip state; direct WS is probed again only after the `WRTG_IP_FAIL_COOLDOWN_SEC` (1 h) mark expires.

## 0.5.24 - 2026-07-19

### Added
- **mipsel support (mips32r2, e.g. MT7621)** — new `wrtg-linux-mipsel` release binary for little-endian 32-bit MIPS routers (Xiaomi Mi Router 3G and similar). Tier-3 Rust target: built with nightly `-Zbuild-std`, `panic=immediate-abort` and a mips32r2 musl cross-gcc; the spurious `libgcc_s.so.1` DT_NEEDED (absent on stock OpenWrt) is stripped with patchelf — only weak crtbegin frame refs pointed into it. `install.sh`/`bootstrap.sh` detect endianness via the ELF EI_DATA byte (uname reports `mips` on both endians); big-endian MIPS stays unsupported. Fixes #6.
- **README version auto-sync** — the release workflow rewrites the `Version / Last updated` line in README.md from the tag and commits it back to main.
- **LuCI docs: CF Worker copy widget** — the Documentation page shows the actual `/etc/wrtg/cf-worker.js` source in a collapsible block with a copy-to-clipboard button and a WRTG_TOKEN hint.

### Fixed
- **Docs vs code drift** — README gained `--version` and `WRTG_MAX_CONNS`; the outdated env advice for `wrtg --check` was removed; `config.default` now warns that `WRTG_FRONT_IP`/`TG_TPROXY_FRONT_IP` only apply on manual runs (init's `-front-ip` wins; SIGHUP reload reads them, restart does not).

## 0.5.23 - 2026-07-19

### Fixed
- **Self-connect DoS** — a direct connection to the listen port (no DNAT, e.g. a LAN port scan) made `SO_ORIGINAL_DST` return the listener's own address, so `blind_relay` chained relays onto itself until the connection semaphore was exhausted and the proxy wedged until restart. Such connections are now dropped (logged once), and self-targets are filtered out of `blind_relay` candidates. Regression test added.
- **Slowloris on handshake classification** — the 750 ms timeout was per-read, so a client dribbling 1 byte/700 ms held a connection slot for ~45 minutes. The init phase now has a 10 s total deadline (also applied to the HTTP request reader).
- **Writer task hang in worker passthrough** — after aborting the up/down tasks, `writer.await` could block forever on an undrained socket, leaking a task per session. The writer is now aborted as in `bridge_ws`.
- **Tail data loss on client close** — `bridge_ws`/`bridge_tcp` aborted the surviving direction instantly on EOF, dropping already-received-but-unwritten frames. The survivor now gets a 2 s drain window before abort.
- **Handshake loss in error branches** — `handle_conn` error paths for `generate_relay_init`/`CryptoCtx::build_direct` called `blind_relay` with an empty prefix instead of the already-read handshake bytes.
- **DoH retry on any error** — `connect_cf_proxy_ws` retried via DoH on *every* failure (including TLS cert errors and HTTP statuses), doubling delays on dead domains. Retry now happens only on transport errors (DNS/connect/timeout).
- **Silent init errors** — unknown CLI arguments exited with code 0; they now exit 2. Added `--version`.
- **Log spam** — the per-connection `tls passthrough` WARN with a ClientHello hex dump (which flooded the 128 KB syslog ring in minutes) is now DEBUG.
- **check-update hangs** — all fetches in `check-update.sh`/`bootstrap.sh` now have 15 s timeouts; temp dirs use `mktemp -d`.
- **LuCI reflected XSS** — the log filter parameter `q` was rendered unescaped; now passed through `entityencode`.
- **bootstrap fallback without verification** — running `install.sh` from the unverified `src.tar.gz` fallback now requires an explicit `WRTG_INSECURE=1`.
- **Config injection** — `install.sh` rejects config values containing `"` `\` `` ` `` `$`; `lib.sh` validates every interface name in `LAN_IF` before it lands in nft `iifname` expressions.
- **setup-nft multi-interface** — `LAN_IF` lists (e.g. `br-lan wt0 br-guest`) now emit one DNAT rule per interface instead of an invalid single `iifname` with several names.
- **zapret calls script** — removed invalid `position 0` from `nft insert` (the rule is inserted at chain start anyway), so `set -e` no longer aborts the apply halfway on nft versions that reject it.

### Changed
- **Lower memory per connection** — relay read buffers 512→128 KiB and the WS channel capacity 256→32; backpressure instead of buffering on stalled clients.
- **Dead code removed** — `tcp_target_ip`, `cf_proxy_domains_for_dc`, `cf_proxy_parallel`, `RawWebSocket::recv`.

## 0.5.22 - 2026-07-18

### Added
- **LuCI one-click update** — Status page can check GitHub for a newer release (atom feed, same approach as `bootstrap.sh`) and install it with a button. Preserves `/etc/wrtg/config`, restarts the service, and shows success/error. Update actions are POST-only with the LuCI auth token. CLI: `/etc/wrtg/check-update.sh check|update`.

## 0.5.21 - 2026-07-17

### Fixed
- **First install ubus noise** — `install.sh` uses `start` when wrtg is not yet running, instead of `restart` (which stops and deletes a procd instance that never existed and prints `Command failed: Not found`).
- **CIDR fetch fallback** — `update-cidr.sh` always creates the official-list temp file before fetch, recreates it if wget/uclient-fetch unlinks on failure, and falls back to built-in defaults without hard-exiting when curl/wget are missing.

## 0.5.20 - 2026-07-16

### Fixed
- **bootstrap: HTTP 403 on `releases/latest`** — `resolve_latest_ver` now reads the tag from the `releases.atom` feed on `github.com` instead of the `api.github.com` REST endpoint, which rate-limits unauthenticated requests to 60/hour per IP (shared/CGNAT ISP addresses routinely hit HTTP 403). The REST API is kept as a fallback, and a clearer error suggests passing an explicit `VER=vX.Y.Z`.

## 0.5.19 - 2026-07-16

### Fixed
- **DoH answer parsing** — `parse_doh_a_records` now confines its scan to the `"Answer"` array instead of walking the whole response body, so glue A-records from `Authority`/`Additional` sections can no longer be picked as the resolved IP on the CF-proxy fallback path (which caused SNI/cert mismatches). Added regression test.

### Changed
- **Dependencies** — bumped Cargo deps (`aes` 0.8→0.9, `ctr` 0.9→0.10, `rand` 0.8→0.10, `socket2` 0.5→0.6, `bytes`, `rustls`) and GitHub Actions (`actions/checkout` v4→v7, `upload`/`download-artifact`, `softprops/action-gh-release`). Migrated the code to the `rand` 0.10 API (`rand::rng()`, `Rng`/`RngExt`, `distr::Alphanumeric`).

## 0.5.18 - 2026-07-14

### Fixed
- **Sticky WS skip-state** - `ip_fail` / `dc_fail` / `ws_blacklist` no longer refresh TTL on every repeated mark (which could tile a cooldown indefinitely under load). Successful CF Worker / CF Proxy / TCP fallback clears skip-state for that DC so normal WS can recover. Stock-front HTTP 302 for DC1/3/5 no longer enters the 45-minute WS blacklist (expected path is CF Worker).

## 0.5.17 — 2026-07-12

### Internal / refactor
- **Deduplicated the code** with no behaviour change: a shared `recrypt()` for the
  three MTProto cipher directions; a generic `ttl_map::TtlMap<K>` backing the
  per-IP/DC/domain cooldown and blacklist maps; a generic `conn_pool::Pool` that
  the direct-WS and CF-worker pools are now thin adapters over (also closing a
  pool size-race leak); one `tls_sni::http_host_raw()` HTTP-Host scanner behind
  the extractors; one `https::get_over()` TLS-GET helper shared by the DoH and
  CF-proxy-list fetches; and a `send_relay_init()` helper for the repeated
  connect→send→bridge steps. Public APIs unchanged.

### CI / Supply chain
- **Fail-closed checksum verification** — `bootstrap.sh` now aborts if
  `sha256sum` is missing or `SHA256SUMS` can't be fetched, instead of silently
  installing unverified. The release binary in the fallback path is now verified
  too (previously unchecked). `WRTG_INSECURE=1` restores the old skip-on-missing
  behaviour; a checksum *mismatch* always aborts.
- **Faster, cached CI** — both workflows use `Swatinem/rust-cache` and install
  `cargo-zigbuild` from a prebuilt binary (`taiki-e/install-action`) instead of
  compiling it from source on every job.
- **CF Worker checks** — CI runs `node --check openwrt/cf-worker.js` and a new
  `openwrt/check-cidr-sync.sh` that fails if the Worker's hardcoded
  `TELEGRAM_CIDRS` drifts from the router-side `default_cidrs()` in `lib.sh`.
- **Dependabot** — weekly grouped updates for Cargo deps and GitHub Actions.
- **Optional Gitea release mirror** — `release.yml` can also publish the
  binaries, bundle, `SHA256SUMS`, and `bootstrap.sh` to the Gitea host that the
  default install path uses; runs only when a `GITEA_TOKEN` secret is set.

### Changed
- **Connection cap** — the accept loop now bounds simultaneously-served
  connections with a semaphore (default 1024, `WRTG_MAX_CONNS`), applying
  backpressure instead of spawning unbounded tasks/buffers under a flood.
- **Non-blocking DC-learn persist** — the learned-IP file append is offloaded to
  the blocking thread pool so it can't stall a reactor worker mid-handshake on
  slow router flash.
- **DoH resolvers are IP-pinned** — Cloudflare/Google/Quad9/AdGuard are now dialed
  on their well-known anycast IPs (SNI/Host unchanged, cert still validated), so
  the DNS-over-HTTPS fallback no longer depends on the system resolver it exists
  to bypass.
- **Bounded rebind backoff** — a persistently unbindable listening socket now
  backs off exponentially (capped at 30s) instead of retrying every ~200ms.
- **Robust redirect / DoH parsing** — WS redirect detection uses the typed
  handshake status code instead of substring-matching the error text, and the
  DoH A-record parser no longer treats `"type":1` as a prefix of `"type":15/16`
  (which could pull CNAME/TXT data into the address list).

### Fixed
- **`uninstall.sh` left files behind** — the cron-removal line
  (`sed … || grep … > tmp && mv`) parsed as `(sed || grep) && mv`; on OpenWrt
  `sed -i` succeeds, so `grep` was skipped, the temp file was never created, and
  `mv` failed — aborting the script under `set -e` **before** the binary/init/
  config were removed. Rewritten as an explicit `if/elif/else`.
- **CF-proxy parallel fallback leaked connections** — the losing race tasks were
  awaited in spawn order (not a real latency race) and, once a winner was found,
  the remaining `JoinHandle`s were dropped, which *detaches* rather than cancels
  them; a sibling that then completed its WSS connect leaked a half-open session.
  Now uses `JoinSet`: first-to-connect wins, losers are aborted, and any that
  connected anyway are closed with a proper close frame.
- **SIGHUP reload data race** — live reload wrote the config file back into the
  process environment via `env::set_var` while worker tasks concurrently read it
  (UB on glibc). Reload now parses the file into a map and builds the config from
  it directly, never mutating the environment. This also makes the file
  authoritative: a key deleted from the config now reverts to its default instead
  of lingering from the previous load.
- **Fronting result was discarded** — when the direct WS path was all-blocked and
  the fronting fallback then failed, the fronting attempt's block/timeout signal
  was computed but ignored, so a DC could be blacklisted for 45 min despite
  fronting showing it was still partially reachable. The two attempts are now
  folded together.
- **`get_original_dst` mis-parsed non-IPv4** — `SO_ORIGINAL_DST` was always read
  as a `sockaddr_in`; a non-`AF_INET` result yielded a garbage IP/port. It now
  checks the address family and reports unknown destinations instead.
- **`blind_relay` could park on a half-open client** — after the remote side
  finished, the client→remote copy task was awaited unconditionally, so a client
  holding its socket open idle kept the task/connection alive forever. It is now
  torn down like the MTProto/TCP bridges (abort + reap).
- **`wrtg --check` ignored the config file** — run by hand it lacks the procd
  environment the daemon starts with, so it reported a configured CF Worker/proxy
  as "none configured". `--check` now seeds its environment from
  `/etc/wrtg/config` first (real env still wins).

## 0.5.16 — 2026-07-10

### Added
- **Live config reload (SIGHUP)** — `/etc/init.d/wrtg reload` now sends SIGHUP
  instead of a full restart. The daemon re-reads `/etc/wrtg/config` and re-applies
  FRONT_IP / front-DCs / Worker+Proxy domains and the DC-learn map **without
  dropping live sessions**. LISTEN / nftables changes still need `restart`.
  LuCI gains a **Save & Reload** button.
- **`wrtg --check` probes every DC** — each of DC1–DC5 is tested over the path it
  actually uses (front for DC2/4, the first CF Worker/Proxy to the real DC IP for
  DC1/3/5), instead of only DC2. Worker/Proxy domains are resolved once up front.
- **DC-learn management in LuCI** — Settings page can add a manual `IP → DC [media]`
  override and clear the learned map; both apply via live reload.

### Fixed
- **CF domain paste** — `CF_WORKER_DOMAIN` / `CF_PROXY_DOMAIN` now accept a full
  URL pasted from the Cloudflare dashboard: `parse_domain_list` strips the
  scheme, path, and trailing slash (`https://w.workers.dev/apiws` → `w.workers.dev`).
  Previously such a value failed TLS SNI ("Name does not resolve") or was silently
  dropped by domain validation, disabling the Worker fallback.
- **Test** — corrected `http_front_passthrough_keeps_dc_host_for_regular_dc`,
  which asserted `parse_http_host` returns a DC-IP host; that helper
  deliberately rejects DC-IP hosts, so the suite failed to build.
- **Empty `WRTG_FRONT_DCS`** now means the default `2,4` (as when unset), not
  "none". The shell config seeds it empty and procd drops empty env vars, so the
  daemon ran on `2,4` — but `wrtg --check` via `set -a && load_config` exported
  the empty string and mis-reported DC2/DC4 as Worker-routed. Use `none` to
  disable fronting.
- **LuCI** — removed the redundant Settings/Logs/Documentation quick-nav row on
  the Status page; it duplicated LuCI's own submenu tabs.

### Changed
- Removed the dead `worker_passthrough_dst` identity helper and the unused
  `orig_ip` argument of `should_try_worker_passthrough` (leftovers from the
  0.5.11–0.5.15 emoji-picker routing changes). No behaviour change.

## 0.5.15 — 2026-07-10

### Fixed
- **Media CDN HTTP emoji** — skip CF Worker passthrough for *all* MTProto-over-HTTP
  (`:80`), including media CDN IPs (`149.154.175.211`, `91.108.56.155`, etc.).
  Worker tunnels to the real DC :80 return HTTP 404; the session closed before
  front fallback, so Android emoji picker API calls on DC1 media :80 failed while
  regular DC2 :80 via `FRONT_IP` worked. Media CDN :80 now uses local front
  passthrough with `kws{N}-1` Host rewrite (same path that already works for TLS).
- **Passthrough logging** — log the actual wire `Host` header (e.g.
  `149.154.167.41:80`) instead of the routing hint `kws{N}.web.telegram.org`.

## 0.5.14 — 2026-07-10

### Fixed
- **Emoji picker HTTP routing** — skip CF Worker passthrough for regular DC
  MTProto-over-HTTP (`Host: <dc-ip>:80`). Worker tunnels to the real DC :80 and
  get HTTP 404; the session closed immediately so front passthrough never ran.
  Regular DC :80 now goes straight to `FRONT_IP` with the original Host header.
  Worker passthrough remains for media CDN / media-alt IPs with `kws{N}-1` Host
  rewrite applied before the tunnel.

## 0.5.13 — 2026-07-10

### Fixed
- **Emoji picker HTTP passthrough** — local `FRONT_IP:80` passthrough no longer
  rewrites `Host` to `kws{N}.web.telegram.org` for regular DC MTProto-over-HTTP.
  The front routes on `Host: <dc-ip>:80`; the `kws{N}` rewrite made it answer
  HTTP 302 (`Location: https://core.telegram.org`) and the standard emoji grid
  stayed empty. Media CDN IPs (`91.108.*`, curated media alt IPs) still get
  `kws{N}-1` rewrite. Worker passthrough tunnels to the real DC IP again (not
  `FRONT_IP:80`, which returns HTTP 400 from the CF edge). Response status
  logged at INFO on passthrough for diagnosis.

## 0.5.12 — 2026-07-10

### Fixed
- **Worker HTTP passthrough to FRONT** — skip tunneling MTProto-over-HTTP :80 to
  `FRONT_IP` via the CF Worker. The front returns HTTP 400 to requests from the
  CF edge (any Host); local front passthrough with `kws{N}` rewrite works from
  the router. Fixes empty emoji picker when worker passthrough "succeeded" but
  relayed 400 responses to clients.

## 0.5.11 — 2026-07-10

### Fixed
- **Worker HTTP passthrough** — tunnel MTProto-over-HTTP to `FRONT_IP:80` (from
  the CF edge) while keeping the client's original `Host: <dc-ip>:80`. Real DC
  :80 returns HTTP 404; rewriting Host to `kws{N}` breaks routing on both
  front and DC. `kws{N}` rewrite remains only for local front passthrough
  fallback.

## 0.5.10 — 2026-07-10

### Fixed
- **Worker HTTP passthrough target** — `worker_passthrough_dst` no longer rewrites
  MTProto-over-HTTP tunnels to `FRONT_IP:80`. The front answers `kws{N}` Host
  headers with HTTP 302, which breaks emoji/sticker API calls; the CF Worker now
  connects to the real DC IP (with Host rewrite applied in `blind_relay`).

## 0.5.9 — 2026-07-10

### Fixed
- **`bootstrap.sh` SHA256SUMS URL** — checksum fetch used a local path (`/tmp/.../SHA256SUMS`), so `curl` failed with `(3) URL rejected`; now downloads from the release asset URL.
- **install.sh non-interactive exit** — interactive_config returned status 1 without a TTY under set -e, so bootstrap install aborted after the banner.
- **OpenWrt install without `install(1)`** — `install.sh` and bootstrap fallback use `cp` + `chmod` via `install_file()` (busybox on many routers has no `install` applet).

## 0.5.8 — 2026-07-10

### Fixed
- **README accuracy** — fallback chain order (`ws_blacklist`/`ip_fail` before direct WS,
  CF Worker sequential not parallel, CF Proxy parallel race), `wrtg --check` scope,
  `WRTG_NO_CFPROXY` also disables worker passthrough, `LISTEN` vs `WRTG_LISTEN`, CIDR/cidr-extra
  merge path, LuCI section, blind-relay classification.

### Changed
- **Docs wording** — neutral media/CDN/passthrough terms in README, LuCI, and config.default (no emoji/sticker mentions).

## 0.5.7 — 2026-07-10

### Changed
- **Single documentation file** — merged `docs/GUIDE.md` into `README.md` (glossary,
  architecture, config, CF Worker/Proxy, troubleshooting). Removed `docs/` directory.
  LuCI Documentation page reads `/etc/wrtg/README.md` deployed by `install.sh`.

## 0.5.6 — 2026-07-10

### Changed
- **Documentation rewrite** — `docs/GUIDE.md` is the single canonical doc (glossary,
  architecture, config, CF Worker/Proxy, troubleshooting). `README.md` is a minimal
  landing page with bootstrap one-liner only. Removed stub docs (`ARCHITECTURE.md`,
  `DEVELOPMENT.md`, `CF_PROXY.md`, `CF_WORKER_SETUP.md`). No Flowseal/Zapret/tg-ws-proxy
  references in user-facing docs.

## 0.5.5 — 2026-07-10

### Changed
- **`bootstrap.sh`** — installs GitHub releases by default (`WRTG_REPO=owner/repo`);
  `WRTG_BASE_URL` / `WRTG_RELEASE_URL` point it at a self-hosted Gitea instead.
  Falls back to release binary + source archive when `wrtg-openwrt.tar.gz` is missing.
- **Install docs** — README and `docs/GUIDE.md` list canonical paths: bootstrap one-liner,
  `ROUTER=… install.sh`, daemon-only update, `SKIP_BUILD=1` without Rust.
- **`make bundle`** — local OpenWrt install bundle + SHA256SUMS (for Gitea/GitHub uploads).
- **`make install-amd64`** — single-arch local install without building arm targets.
- **`install.sh`** — LAN_IF prompt default is auto-detect (empty), not `eth0`.

## 0.5.4 — 2026-07-10

### Fixed
- **OpenWrt tuning env vars** — `WRTG_CFPROXY_*`, `WRTG_DOH_CACHE_SEC`, `WRTG_WS_PING_SEC`,
  and `WRTG_TCP_KEEPALIVE_SEC` from `/etc/wrtg/config` are now loaded by `lib.sh`, passed to
  procd in `wrtg.init`, and exported for LuCI `wrtg --check`.

## 0.5.3 — 2026-07-10

### Changed
- **Documentation consolidated** into single `docs/GUIDE.md` (architecture, CF Worker/Proxy,
  env vars, release checks). Old docs stubbed with redirects; LuCI docs viewer simplified.
- **`install.sh` Windows deploy** — `SKIP_BUILD=1` accepts a non-executable `dist/` binary (NTFS/Git Bash).

## 0.5.2 — 2026-07-10

### Added
- **TLS fronting fallback** — opt-in via `WRTG_FRONTING_SNI`: connect to target IP with
  Host `kws{N}.web.telegram.org` but alternate TLS SNI; cooldown
  `WRTG_FRONTING_COOLDOWN_SEC` (default 1800s) after failure. Runs after direct WS,
  before CF fallback.
- **`wrtg --check`** — connectivity diagnostics: DNS, direct WSS via FRONT_IP, CF Worker
  and CF Proxy WSS probes; exit 0/1.
- **`dc_fail_until` + adaptive WS timeout** — after WS failure on a DC, shorter connect
  timeout (2s vs 5s) for 60s.

### Env
- `WRTG_FRONTING_SNI`, `WRTG_FRONTING_COOLDOWN_SEC`
- `WRTG_DC_FAIL_COOLDOWN_SEC`, `WRTG_WS_FAIL_TIMEOUT_SEC`, `WRTG_WS_FAIL_TIMEOUT_FAST_SEC`

## 0.5.1 — 2026-07-10

### Added
- **CF proxy DoH fallback** — when hostname dial fails, resolve the base domain via
  DoH (Cloudflare / Google / Quad9 / AdGuard race + 5 min cache) and retry with
  IP + SNI.
- **HTTP 429 cooldown per CF proxy domain** — exponential backoff 45s→300s;
  cooled-down domains are skipped in `try_cf_fallback`.
- **Parallel CF proxy fallback** — primary domain sequential, remaining domains
  raced in parallel (semaphore via `WRTG_CFPROXY_PARALLEL`, default 2).
- **WS idle ping + TCP keepalive** — bridge sends WebSocket ping every 30s;
  relay sockets get TCP keepalive (30s default).
- **Sticky CF proxy domain per DC** — successful CF proxy connect updates the
  balancer preference for that DC.

### Env
- `WRTG_CFPROXY_429_COOLDOWN_SEC`, `WRTG_CFPROXY_429_MAX_COOLDOWN_SEC`,
  `WRTG_CFPROXY_PARALLEL`, `WRTG_DOH_CACHE_SEC`, `WRTG_WS_PING_SEC`,
  `WRTG_TCP_KEEPALIVE_SEC`

## 0.5.0 — 2026-07-09

### Security
- Enabled public-root TLS certificate validation for every WSS/HTTPS connection.
- Validate the complete WebSocket upgrade and safely handle fragmented/oversized frames.
- Added `openwrt/cf-worker.js`: Telegram CIDR/port allowlist plus optional
  `WRTG_TOKEN` / `WRTG_CF_WORKER_TOKEN` authentication.
- LuCI service actions now require POST + session token; raw config is syntax-checked.
- `bootstrap.sh` verifies the release bundle SHA256.

### Fixed
- WS/TCP bridges terminate when either direction closes; worker initial-send
  failures can try the next Worker.
- Direct pool no longer skips startup or creates unused media connections.
- CF Worker pool size is bounded per `(DC, media)` instead of multiplied by the
  number of Worker domains.
- Public CF Proxy pool is opt-in and limited to three attempts per connection.
- Strict `dc_learn` IPv4 parsing, media corrections, and admin-file precedence.
- LAN interface/IP auto-detection, transactional CIDR/nft updates, complete LuCI uninstall.
- OpenWrt `reload` now performs the restart required to apply file config.

### Changed
- Removed duplicated Worker/Proxy/config/nft documentation artifacts.
- Optional zapret/calls helpers remain in the repository but are no longer
  installed or invoked by wrtg core.
- Consolidated current development documentation; release history lives here.

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
- **Worker passthrough for media** — TLS / MTProto-over-HTTP media traffic that can't be MTProto-bridged (so it would `blind_relay` to the front, which returns HTTP 302) now tunnels through the CF Worker to the **real DC IP:port**. Fixes media/CDN loading on transparent-mode networks where only the front is reachable. Requires the Worker to honour the `port` query param (see below); falls back to front passthrough if the Worker is unreachable. Disable with `WRTG_NO_WORKER_PASSTHROUGH=1`.
- **CF Worker `port` param** — `wss://<worker>/apiws?dst=IP&dc=N&port=P` (default 443). Worker source is now maintained in `openwrt/cf-worker.js`; backward-compatible (existing MTProto path unaffected).
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
- **Docs** — README rewritten (quickstart-first, current); CF_WORKER_SETUP covers media passthrough; ARCHITECTURE shows the worker-passthrough branch.

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
