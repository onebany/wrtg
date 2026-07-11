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

/// Build config from the process environment (startup path).
pub fn load_from_env() -> WrtgConfig {
    load_from(&|k| env::var(k).ok())
}

/// Build config from a parsed `KEY=VALUE` map (SIGHUP reload path). Keeps the
/// config file as the single source of truth and avoids mutating the process
/// environment while worker tasks concurrently read it.
pub fn load_from_map(map: &HashMap<String, String>) -> WrtgConfig {
    load_from(&|k| map.get(k).cloned())
}

/// Shared config builder parameterized by a key lookup, so the same parsing and
/// defaults serve both the env (startup) and the config-file map (reload).
fn load_from(get: &dyn Fn(&str) -> Option<String>) -> WrtgConfig {
    let mut front_ip = get("WRTG_FRONT_IP")
        .or_else(|| get("FRONT_IP"))
        .or_else(|| get("TG_TPROXY_FRONT_IP"))
        .unwrap_or_else(|| "149.154.167.220".to_string());
    front_ip = front_ip.trim_matches('\r').trim().to_string();

    let listen_addr = get("WRTG_LISTEN")
        .unwrap_or_else(|| "0.0.0.0:8443".to_string())
        .trim_matches('\r')
        .to_string();

    let dc_front_ips = parse_dc_front_ips_with(get);
    let front_dcs = load_front_dcs_with(get);
    let cf_worker_domains = load_worker_domains_with(get);
    let cf_proxy_domains = load_proxy_domains_with(get);

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
/// fronts DC2/DC4). `all`/`*` → 1-5; `none` → direct-only for all DCs.
///
/// An empty value is treated as unset (→ default `2,4`), not as `none`: the
/// shell config seeds `WRTG_FRONT_DCS=""` and procd drops empty env vars, so the
/// daemon runs on the default — but `wrtg --check` via `set -a && load_config`
/// exports the empty string. Treating empty as the default keeps both paths in
/// agreement. Use `none` to actually disable fronting.
fn load_front_dcs_with(get: &dyn Fn(&str) -> Option<String>) -> Vec<i32> {
    match get("WRTG_FRONT_DCS") {
        Some(v) if !v.trim().trim_matches('\r').trim().is_empty() => parse_front_dcs(&v),
        _ => vec![2, 4],
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

fn load_worker_domains_with(get: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut domains = Vec::new();
    if let Some(v) = get("CF_WORKER_DOMAIN") {
        domains.extend(parse_domain_list(&v));
    }
    if let Some(v) = get("WRTG_CF_WORKER_DOMAINS") {
        domains.extend(parse_domain_list(&v));
    }
    normalize_domain_pool(&domains)
}

fn load_proxy_domains_with(get: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut domains = Vec::new();
    if let Some(v) = get("CF_PROXY_DOMAIN") {
        domains.extend(parse_domain_list(&v));
    }
    if let Some(v) = get("WRTG_CF_PROXY_DOMAINS") {
        domains.extend(parse_domain_list(&v));
    }
    normalize_domain_pool(&domains)
}

pub fn parse_dc_front_ips() -> HashMap<i32, String> {
    parse_dc_front_ips_with(&|k| env::var(k).ok())
}

fn parse_dc_front_ips_with(get: &dyn Fn(&str) -> Option<String>) -> HashMap<i32, String> {
    let mut map = HashMap::new();

    if let Some(raw) = get("WRTG_DC_IPS") {
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
        if let Some(ip) = get(&key) {
            let ip = ip.trim().trim_matches('\r');
            if !ip.is_empty() {
                map.insert(dc, ip.to_string());
            }
        }
    }
    if let Some(ip) = get("DC203_FRONT_IP") {
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

const DEFAULT_CONFIG_FILE: &str = "/etc/wrtg/config";

pub fn config_file_path() -> String {
    env::var("WRTG_CONFIG_FILE").unwrap_or_else(|_| DEFAULT_CONFIG_FILE.to_string())
}

/// Parse `KEY=VALUE` lines from the shell config file into a map. Used by the
/// SIGHUP reload: the config file is the source of truth, and building the new
/// config from this map (rather than round-tripping through `env::set_var`)
/// avoids a data race with worker tasks that read the environment concurrently,
/// and makes the file authoritative — a key deleted from the file reverts to its
/// default instead of lingering from the previous load.
/// Approximates shell quoting well enough for the values LuCI / the defaults write.
pub fn import_config_file(path: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in text.lines() {
        if let Some((key, val)) = parse_kv_line(line) {
            map.insert(key.to_string(), val.to_string());
        }
    }
    map
}

/// Parse one shell `KEY=VALUE` config line. `None` for blanks/comments/invalid
/// keys. Approximates shell quoting: strips one layer of matching quotes, else
/// takes the first whitespace-delimited token (dropping trailing comments).
fn parse_kv_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, raw) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let raw = raw.trim();
    let val = if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        &raw[1..raw.len() - 1]
    } else {
        raw.split_whitespace().next().unwrap_or("")
    };
    Some((key, val))
}

/// Re-read the config file and re-apply front/domains + reload the DC-learn map,
/// without touching the listener. Triggered by SIGHUP (`/etc/init.d/wrtg reload`).
pub fn reload_from_file() {
    let path = config_file_path();
    let map = import_config_file(&path);
    let keys = map.len();
    let cfg = load_from_map(&map);
    apply_config(&cfg);
    crate::dc_learn::load();
    log::info!(
        "reloaded config from {path} ({keys} keys): front-ip={} front-dcs={:?} cf-workers={} cf-proxies={}",
        cfg.front_ip,
        cfg.front_dcs,
        cfg.cf_worker_domains.len(),
        cfg.cf_proxy_domains.len()
    );
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

    #[test]
    fn parse_kv_line_cases() {
        assert_eq!(
            parse_kv_line("FRONT_IP=149.154.167.220"),
            Some(("FRONT_IP", "149.154.167.220"))
        );
        assert_eq!(
            parse_kv_line("CF_WORKER_DOMAIN=\"a.dev,b.dev\""),
            Some(("CF_WORKER_DOMAIN", "a.dev,b.dev"))
        );
        // Unquoted value with a trailing comment ends at the first token.
        assert_eq!(
            parse_kv_line("WRTG_WS_POOL_SIZE=2   # default"),
            Some(("WRTG_WS_POOL_SIZE", "2"))
        );
        assert_eq!(parse_kv_line("  # a comment"), None);
        assert_eq!(parse_kv_line(""), None);
        assert_eq!(parse_kv_line("not a config line"), None);
        assert_eq!(parse_kv_line("EMPTY="), Some(("EMPTY", "")));
    }

    #[test]
    fn load_from_map_reads_values_and_defaults() {
        let mut map = HashMap::new();
        map.insert("FRONT_IP".to_string(), "10.0.0.1".to_string());
        map.insert("WRTG_FRONT_DCS".to_string(), "1,3".to_string());
        map.insert("WRTG_DC_IPS".to_string(), "2:5.5.5.5".to_string());

        let cfg = load_from_map(&map);
        assert_eq!(cfg.front_ip, "10.0.0.1");
        assert_eq!(cfg.front_dcs, vec![1, 3]);
        assert_eq!(
            cfg.dc_front_ips.get(&2).map(String::as_str),
            Some("5.5.5.5")
        );
        // Absent listen key falls back to the built-in default.
        assert_eq!(cfg.listen_addr, "0.0.0.0:8443");
    }

    #[test]
    fn load_from_map_is_authoritative_dropped_key_reverts_to_default() {
        // The reload path builds config purely from the file map, so a key that
        // is no longer present reverts to its default rather than lingering.
        let empty = HashMap::new();
        let cfg = load_from_map(&empty);
        assert_eq!(cfg.front_ip, "149.154.167.220");
        assert_eq!(cfg.front_dcs, vec![2, 4]);
        assert!(cfg.dc_front_ips.is_empty());
    }
}
