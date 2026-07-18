# wrtg — developer notes

## Module map (`crates/wrtg/src/`)

| Module | Role |
|--------|------|
| `main`, `handshake`, `mtproto` | Accept, classification, crypto |
| `bridge`, `ws`, `tls` | Relay, WSS, TLS passthrough |
| `ws_pool`, `cf_worker_pool` | Bounded connection pools |
| `ws_blacklist`, `ip_fail` | TTL blacklist, FRONT_IP cooldown |
| `dc_learn` | IP → DC mapping |
| `cf_proxy`, `cf_balancer`, `cf_proxy_domains` | CF Proxy fallback |
| `config`, `watchdog`, `sockopt` | Startup, listener recovery, transparent socket |

## Recent behavior changes

### 0.5.22

- **LuCI / on-router update** — `/etc/wrtg/check-update.sh` resolves the latest
  GitHub release via `releases.atom` (API fallback) and can install the OpenWrt
  bundle in place while keeping `/etc/wrtg/config`. LuCI Status exposes Check /
  Update as POST actions with the session auth token. No new daemon env vars.

### 0.5.19

- **DoH answer parsing** — `parse_doh_a_records` is now scoped to the `"Answer"`
  array; glue A-records in `Authority`/`Additional` are ignored so the CF-proxy
  fallback can't dial a non-answer IP under the original SNI/Host.
- **Dep bumps** — `rand` moved to 0.10 (`rand::rng()`, `Rng`/`RngExt`,
  `distr::Alphanumeric`); `aes` 0.9 / `ctr` 0.10 / `socket2` 0.6 updated. No
  public API or behaviour change beyond the DoH fix.

### 0.5.18

- **WS skip-state recovery** — cooldown/blacklist entries no longer extend their TTL
  on repeated marks; successful CF Worker / CF Proxy / TCP fallback clears
  `ip_fail`, `dc_fail`, and `ws_blacklist` for that DC. HTTP 302 on the stock
  front for DC1/3/5 no longer triggers the 45-minute WS blacklist (expected → use
  CF Worker).

### 0.5.13

- **MTProto-over-HTTP `:80` Host header** — local `FRONT_IP` passthrough keeps the
  client's `Host: <dc-ip>:80` for regular DCs; `kws{N}` rewrite applies only to
  blocked media CDN / curated media alt IPs. Worker passthrough tunnels to the
  real DC IP (not `FRONT_IP:80`). Passthrough responses log HTTP status at INFO.

### 0.5.2

- **`wrtg --check`** — standalone connectivity probe; does not start the daemon.
- **TLS fronting** — set `WRTG_FRONTING_SNI` to enable fallback between direct WS and CF.
- **Adaptive WS timeout** — after DC WS failure, connect timeout drops to
  `WRTG_WS_FAIL_TIMEOUT_FAST_SEC` for `WRTG_DC_FAIL_COOLDOWN_SEC`.

### 0.5.0

- WSS/HTTPS requires a valid public TLS certificate.
- Public CF Proxy auto-fetch defaults to off (`WRTG_CFPROXY_AUTO=1` opts in).
- `/etc/init.d/wrtg reload` performs restart instead of sending ineffective SIGHUP.
- Direct WS pool is non-media and only prewarms fronted DCs.
- CF Worker pool size is total per `(DC, media)`, not multiplied by Worker count.
- New optional `WRTG_CF_WORKER_TOKEN` header secret.
- `dc-ips.txt` overrides learned mappings on conflict.

## Build

```sh
cargo build --release -p wrtg
cargo test -p wrtg
./build-rust.sh amd64   # static musl for OpenWrt
```

Output: `dist/wrtg-linux-{amd64,arm64,arm}`
