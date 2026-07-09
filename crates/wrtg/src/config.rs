//! Runtime configuration loading at process startup.

use std::collections::HashMap;
use std::env;

use crate::cf_balancer::{parse_domain_list, set_proxy_domains, set_worker_domains};
use crate::cf_proxy_domains::normalize_domain_pool;
use crate::mtproto::{set_dc_front_ips, set_front_dcs, set_front_ip};

#[derive(Debug, Clone)]
pub struct WrtgConfig {
    pub listen_addr: String,
    pub front_ip: String,
    pub dc_front_ips: HashMap<i32, String>,
    pub front_dcs: Vec<i32>,
    pub cf_worker_domains: Vec<String>,
    pub cf_proxy_domains: Vec<String>,
}

pub fn load_from_env() -> WrtgConfig {
    let mut front_ip = env::var("WRTG_FRONT_IP")
        .or_else(|_| env::var("FRONT_IP"))
        .or_else(|_| env::var("TG_TPROXY_FRONT_IP"))
        .unwrap_or_else(|_| "149.154.167.220".to_string());
    front_ip = front_ip.trim_matches('\r').trim().to_string();

    let listen_addr = env::var("WRTG_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:8443".to_string())
        .trim_matches('\r')
        .to_string();

    let dc_front_ips = parse_dc_front_ips();
    let front_dcs = load_front_dcs();
    let cf_worker_domains = load_worker_domains();
    let cf_proxy_domains = load_proxy_domains();

    WrtgConfig {
        listen_addr,
        front_ip,
        dc_front_ips,
        front_dcs,
        cf_worker_domains,
        cf_proxy_domains,
    }
}

/// DCs the global FRONT_IP applies to. Default `2,4` (the stock front only
/// fronts DC2/DC4). `all`/`*` → 1-5; `none`/empty → direct-only for all DCs.
fn load_front_dcs() -> Vec<i32> {
    match env::var("WRTG_FRONT_DCS") {
        Ok(v) => parse_front_dcs(&v),
        Err(_) => vec![2, 4],
    }
}

pub fn parse_front_dcs(raw: &str) -> Vec<i32> {
    let v = raw.trim().trim_matches('\r').trim();
    if v.eq_ignore_ascii_case("all") || v == "*" {
        return vec![1, 2, 3, 4, 5];
    }
    if v.is_empty() || v.eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    let mut out: Vec<i32> = v
        .split([',', ';', ' '])
        .filter_map(|s| s.trim().parse::<i32>().ok())
        .filter(|dc| (1..=5).contains(dc) || *dc == 203)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn load_worker_domains() -> Vec<String> {
    let mut domains = Vec::new();
    if let Ok(v) = env::var("CF_WORKER_DOMAIN") {
        domains.extend(parse_domain_list(&v));
    }
    if let Ok(v) = env::var("WRTG_CF_WORKER_DOMAINS") {
        domains.extend(parse_domain_list(&v));
    }
    normalize_domain_pool(&domains)
}

fn load_proxy_domains() -> Vec<String> {
    let mut domains = Vec::new();
    if let Ok(v) = env::var("CF_PROXY_DOMAIN") {
        domains.extend(parse_domain_list(&v));
    }
    if let Ok(v) = env::var("WRTG_CF_PROXY_DOMAINS") {
        domains.extend(parse_domain_list(&v));
    }
    normalize_domain_pool(&domains)
}

pub fn parse_dc_front_ips() -> HashMap<i32, String> {
    let mut map = HashMap::new();

    if let Ok(raw) = env::var("WRTG_DC_IPS") {
        for part in raw.split([',', ';']) {
            let part = part.trim();
            if let Some((dc_s, ip)) = part.split_once(':') {
                if let Ok(dc) = dc_s.trim().parse::<i32>() {
                    let ip = ip.trim().trim_matches('\r');
                    if !ip.is_empty() {
                        map.insert(dc, ip.to_string());
                    }
                }
            }
        }
    }

    for dc in 1..=5 {
        let key = format!("DC{dc}_FRONT_IP");
        if let Ok(ip) = env::var(&key) {
            let ip = ip.trim().trim_matches('\r');
            if !ip.is_empty() {
                map.insert(dc, ip.to_string());
            }
        }
    }
    if let Ok(ip) = env::var("DC203_FRONT_IP") {
        let ip = ip.trim().trim_matches('\r');
        if !ip.is_empty() {
            map.insert(203, ip.to_string());
        }
    }

    map
}

pub fn apply_config(cfg: &WrtgConfig) {
    set_front_ip(cfg.front_ip.clone());
    set_dc_front_ips(cfg.dc_front_ips.clone());
    set_front_dcs(cfg.front_dcs.clone());

    set_worker_domains(cfg.cf_worker_domains.clone());

    set_proxy_domains(cfg.cf_proxy_domains.clone());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_front_dcs_cases() {
        assert_eq!(parse_front_dcs("2,4"), vec![2, 4]);
        assert_eq!(parse_front_dcs(" 4 ; 2 "), vec![2, 4]);
        assert_eq!(parse_front_dcs("all"), vec![1, 2, 3, 4, 5]);
        assert_eq!(parse_front_dcs("*"), vec![1, 2, 3, 4, 5]);
        assert!(parse_front_dcs("none").is_empty());
        assert!(parse_front_dcs("").is_empty());
        assert_eq!(parse_front_dcs("2,2,4"), vec![2, 4]); // dedup
        assert_eq!(parse_front_dcs("0,2,6,203"), vec![2, 203]);
    }
}
