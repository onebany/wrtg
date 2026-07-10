//! Connectivity diagnostics (`wrtg --check`).

use std::time::{Duration, Instant};

use tokio::time::timeout;

use crate::cf_balancer::{proxy_domains, worker_domains};
use crate::cf_proxy::cf_proxy_ws_domain;
use crate::config::WrtgConfig;
use crate::mtproto::{dc_default_ip, ws_domains};
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

async fn probe_cf_worker(worker: &str, dst_ip: &str) -> Result<(), String> {
    connect_cf_worker_ws(worker, dst_ip, 2, PROBE_TIMEOUT)
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
    let front = cfg.front_ip.clone();
    let dst_dc2 = dc_default_ip(2).unwrap_or("149.154.167.51").to_string();

    eprintln!("============================================================");
    eprintln!("  wrtg connectivity check");
    eprintln!("============================================================");

    eprintln!("\nDirect WSS (DC2 via FRONT_IP):");
    for domain in ws_domains(2, false) {
        let label = format!("{domain} @ {front}");
        probes.push(probe_wss(&label, probe_direct_wss(&front, &domain)).await);
    }

    let workers = worker_domains();
    if !workers.is_empty() {
        eprintln!("\nCloudflare Worker (DC2 WSS probe):");
        for worker in &workers {
            let label = format!("{worker}/apiws?dst={dst_dc2}");
            probes.push(probe_wss(&label, probe_cf_worker(worker, &dst_dc2)).await);
            probes.push(probe_dns(worker).await);
        }
    } else {
        eprintln!("\nCloudflare Worker: not configured (skip)");
    }

    let proxies = if cfg.cf_proxy_domains.is_empty() {
        proxy_domains()
    } else {
        cfg.cf_proxy_domains.clone()
    };
    if !proxies.is_empty() {
        eprintln!("\nCloudflare Proxy (DC2 WSS probe):");
        for cf_domain in &proxies {
            let ws_host = cf_proxy_ws_domain(cf_domain, 2, false);
            let label = ws_host.clone();
            probes.push(probe_wss(&label, probe_cf_proxy(cf_domain, &ws_host)).await);
            probes.push(probe_dns(cf_domain).await);
        }
    } else {
        eprintln!("\nCloudflare Proxy: not configured (skip)");
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
