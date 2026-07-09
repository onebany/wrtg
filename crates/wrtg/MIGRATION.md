# Go → Rust migration map

| Go (removed `wrtgo` / `tg_wrt/legacy/tproxy-go`) | Rust (`crates/wrtg/src/`) | Status |
|---|---|---|
| `main.go` | `main.rs` | Ported |
| `mtproto.go` | `mtproto.rs` | Ported |
| `handshake_read.go` | `handshake.rs` | Ported |
| `bridge.go` | `bridge.rs` | Ported |
| `websocket.go` | `ws.rs` | Ported |
| `splitter.go` | `splitter.rs` | Ported |
| `tls_sni.go` | `tls_sni.rs` | Ported |
| `media_cdn.go` | `media.rs` | Ported |
| `ws_blacklist.go` | `ws_blacklist.rs` | Ported |
| — | `ws_pool.rs` | New in v0.2 |
| — | `cf_worker_pool.rs` | New in v0.3 |
| — | `cf_proxy.rs` / `cf_balancer.rs` | New in v0.3 |
| — | `ip_fail.rs` | New in v0.3 |
| — | `config.rs` / `watchdog.rs` | New in v0.3 |
| — | `dc_learn.rs` | New in v0.4.4 |
| — | `tls.rs` | Verified TLS configuration in v0.5.0 |
| `sockopt_linux.go` | `sockopt/linux.rs` | Ported |
| `sockopt_other.go` | `sockopt/stub.rs` | Ported |
| `sockopt.go` | `sockopt/mod.rs` | Ported |

## Notes

- **CF Worker / CfProxy**: implemented in v0.3; Worker source is
  `openwrt/cf-worker.js`.
- **MTProxy secret mode**: not supported (direct obfuscated2 only, same as Go).
- **Runtime `front-ip`**: `--front-ip`, `WRTG_FRONT_IP`, `FRONT_IP`, or `/etc/wrtg/config` on OpenWrt.

## 0.5.0 behavior changes

- WSS/HTTPS now requires a valid public TLS certificate.
- Public CF Proxy auto-fetch defaults to off (`WRTG_CFPROXY_AUTO=1` opts in).
- `/etc/init.d/wrtg reload` performs restart instead of sending ineffective SIGHUP.
- Direct WS pool is non-media and only prewarms fronted DCs.
- CF Worker pool size is total per `(DC, media)`, not multiplied by Worker count.
- New optional `WRTG_CF_WORKER_TOKEN` header secret; deploy the matching Worker
  source and `WRTG_TOKEN` secret before enabling it.
- `dc-ips.txt` now overrides learned mappings on conflict.

## Test parity

| Go test | Rust test |
|---|---|
| `handshake_read_test.go` | `handshake.rs` — `looks_like_tls_stream_only_at_start` |
| `tls_sni_test.go` | `tls_sni.rs` — SNI, HTTP Host, passthrough targets |
| `media_cdn_test.go` | `media.rs` — Host rewrite, CDN passthrough |
| `mtproto_test.go` | `mtproto.rs` — `ws_target_ip_*` |

## Build targets

```sh
# Native (dev / Windows)
cargo build --release -p wrtg
cargo test -p wrtg

# OpenWrt / router (static musl)
./build-rust.sh amd64
./build-rust.sh arm64
./build-rust.sh arm

# Or via Makefile
make rust-amd64
make rust-arm64
make rust-arm
```

Output: `dist/wrtg-linux-{amd64,arm64,arm}`

## TCP fallback regression (fixed)

A prior Rust port mistakenly used `dc_default_ip(dc)` for TCP fallback after WS 302/blacklist
(e.g. DC1 → `149.154.175.50` / blocked `149.154.175.53`). Go uses `wsTargetIP()` → `FRONT_IP`
(`149.154.167.220`) via `tcpFallbackTargets()` → `add(wsTargetIP(...))`.

Rust now matches: `tcp_fallback_targets()` calls `ws_target_ip()`, which returns `front_ip()`
when set. Test: `tcp_fallback_uses_front_ip`.

HTTP :80 media passthrough also had a Rust-only bug: `parse_http_host()` accepted
`Host: <dc-ip>:80` as a hostname (Go rejects IP hosts), so `http_front_host()` was skipped
for logging/routing hints. Fixed to strip numeric ports and reject IP literals like Go.

Go implementation removed 2026-07-07. Historical reference: `tg_wrt/legacy/tproxy-go` (superseded by wrtg).
