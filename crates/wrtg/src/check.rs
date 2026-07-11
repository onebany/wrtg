//! Connectivity diagnostics (`wrtg --check`).

use std::time::{Duration, Instant};

use tokio::time::timeout;

use crate::cf_balancer::{proxy_domains, worker_domains};
use crate::cf_proxy::cf_proxy_ws_domain;
use crate::config::WrtgConfig;
use crate::mtproto::{dc_default_ip, dc_front_ip, front_applies_to_dc, ws_domains};
use crate::ws::{connect_cf_worker_ws, connect_ws, connect_ws_with_headers, WsConnectError};

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

struct Probe {
    label: String,
    ok: bool,
    detail: String,
    ms: u128,
}

impl Probe {
    fn ok(label: impl Into<String>, detail: impl Into<String>, ms: u128) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            ok: true,
            ms,
        }
    }

    fn fail(label: impl Into<String>, detail: impl Into<String>, ms: u128) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
            ok: false,
            ms,
        }
    }
}

async fn probe_dns(host: &str) -> Probe {
    let label = format!("DNS {host}");
    let start = Instant::now();
    match timeout(PROBE_TIMEOUT, tokio::net::lookup_host((host, 443))).await {
        Ok(Ok(mut addrs)) => {
            let first = addrs.next().map(|a| a.ip().to_string());
            match first {
                Some(ip) => Probe::ok(label, ip, start.elapsed().as_millis()),
                None => Probe::fail(label, "no addresses", start.elapsed().as_millis()),
            }
        }
        Ok(Err(e)) => Probe::fail(label, e.to_string(), start.elapsed().as_millis()),
        Err(_) => Probe::fail(label, "timeout", start.elapsed().as_millis()),
    }
}

async fn probe_wss(
    label: &str,
    connect: impl std::future::Future<Output = Result<(), String>>,
) -> Probe {
    let start = Instant::now();
    match connect.await {
        Ok(()) => Probe::ok(label, "WSS handshake OK", start.elapsed().as_millis()),
        Err(e) => Probe::fail(label, e, start.elapsed().as_millis()),
    }
}

async fn probe_direct_wss(target_ip: &str, domain: &str) -> Result<(), String> {
    connect_ws(target_ip, domain, "/apiws", PROBE_TIMEOUT)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

async fn probe_cf_worker(worker: &str, dst_ip: &str, dc: i32) -> Result<(), String> {
    connect_cf_worker_ws(worker, dst_ip, dc, PROBE_TIMEOUT)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

async fn probe_cf_proxy(cf_domain: &str, ws_host: &str) -> Result<(), String> {
    connect_ws_with_headers(cf_domain, ws_host, "/apiws", PROBE_TIMEOUT, &[])
        .await
        .map(|_| ())
        .map_err(|e| match e {
            WsConnectError::Handshake(h) => format!("HTTP {}", h.status_code),
            other => other.into_io().to_string(),
        })
}

fn print_probe(p: &Probe) {
    let status = if p.ok { "OK  " } else { "FAIL" };
    eprintln!("  {:<42} ... [{status}]  {}ms", p.label, p.ms);
    if !p.ok {
        eprintln!("      {}", p.detail);
    }
}

pub async fn run_check(cfg: &WrtgConfig) -> i32 {
    let mut probes: Vec<Probe> = Vec::new();

    eprintln!("============================================================");
    eprintln!("  wrtg connectivity check");
    eprintln!("============================================================");

    let workers = worker_domains();
    let proxies = if cfg.cf_proxy_domains.is_empty() {
        proxy_domains()
    } else {
        cfg.cf_proxy_domains.clone()
    };

    // Resolve each configured Worker / Proxy domain once.
    if !workers.is_empty() || !proxies.is_empty() {
        eprintln!("\nDNS resolution (Worker / Proxy domains):");
        for d in workers.iter().chain(proxies.iter()) {
            probes.push(probe_dns(d).await);
        }
    } else {
        eprintln!("\nCloudflare Worker / Proxy: none configured");
    }

    // Probe every DC over the path it actually uses: fronted DCs (default 2/4)
    // go direct WSS via the front IP; the rest tunnel through the first CF
    // Worker (else the first CF Proxy) to the real DC IP.
    eprintln!("\nPer-DC path probe:");
    for dc in [1, 2, 3, 4, 5] {
        let domain = ws_domains(dc, false)
            .into_iter()
            .next()
            .unwrap_or_else(|| format!("dc{dc}"));
        if front_applies_to_dc(dc) {
            let ip = dc_front_ip(dc);
            let label = format!("DC{dc} front {domain} @ {ip}");
            probes.push(probe_wss(&label, probe_direct_wss(&ip, &domain)).await);
        } else if let Some(worker) = workers.first() {
            let dst = dc_default_ip(dc).unwrap_or("").to_string();
            let label = format!("DC{dc} worker {worker} -> {dst}");
            probes.push(probe_wss(&label, probe_cf_worker(worker, &dst, dc)).await);
        } else if let Some(cf_domain) = proxies.first() {
            let ws_host = cf_proxy_ws_domain(cf_domain, dc, false);
            let label = format!("DC{dc} proxy {ws_host}");
            probes.push(probe_wss(&label, probe_cf_proxy(cf_domain, &ws_host)).await);
        } else {
            eprintln!("  DC{dc}: no worker/proxy — direct-blocked DC will fail on this network");
        }
    }

    if let Some(sni) = crate::fronting::fronting_sni() {
        eprintln!("\nTLS fronting SNI configured: {sni} (runtime fallback only)");
    }

    eprintln!("\nResults:");
    for p in &probes {
        print_probe(p);
    }

    let all_ok = probes.iter().all(|p| p.ok);
    eprintln!("\n============================================================");
    if all_ok {
        eprintln!("  Result: all checks passed");
    } else {
        let n = probes.iter().filter(|p| !p.ok).count();
        eprintln!("  Result: {n} check(s) failed");
    }
    eprintln!("============================================================");

    if all_ok {
        0
    } else {
        1
    }
}
