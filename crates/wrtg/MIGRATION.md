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
