//! Self-learning IP → DC map.
//!
//! The hardcoded tables in [`crate::mtproto`] only cover a handful of Telegram
//! datacenter IPs. Telegram adds/rotates IPs over time, and some clients
//! (notably Telegram for Android) do **not** embed the DC number in the
//! obfuscated handshake — so a connection to a fresh DC IP would fall to a slow
//! blind passthrough via the CF worker instead of the fast front.
//!
//! This module closes that gap without a rebuild:
//!   * connections that **do** embed a valid DC teach us `orig_ip → (dc, media)`,
//!   * connections that **don't** resolve the IP from what we've already learned,
//!   * learned entries persist to a file so they survive restarts,
//!   * an admin-editable file (`/etc/wrtg/dc-ips.txt`) is loaded at startup.
//!
//! File format (both files), one entry per line, `#` comments allowed:
//! ```text
//! 149.154.167.35 2          # DC2, non-media
//! 149.154.171.255 5 media   # DC5, media endpoint
//! ```

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{LazyLock, RwLock};

use crate::mtproto::{dc_from_hardcoded, valid_dc};

static LEARNED: LazyLock<RwLock<HashMap<String, (i32, bool)>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

const DEFAULT_LEARN_FILE: &str = "/etc/wrtg/dc-ips-learned.txt";
const DEFAULT_MANUAL_FILE: &str = "/etc/wrtg/dc-ips.txt";

fn learn_file() -> String {
    std::env::var("WRTG_DC_LEARN_FILE").unwrap_or_else(|_| DEFAULT_LEARN_FILE.to_string())
}

fn manual_file() -> String {
    std::env::var("WRTG_DC_IPS_FILE").unwrap_or_else(|_| DEFAULT_MANUAL_FILE.to_string())
}

/// Parse one `IP DC [media]` line. Returns `None` for blanks/comments/garbage.
fn parse_line(line: &str) -> Option<(String, i32, bool)> {
    let line = line.split('#').next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    let mut it = line.split_whitespace();
    let ip = it.next()?;
    // Cheap IPv4 sanity check (4 dotted octets); avoids poisoning the map with junk.
    if ip.split('.').filter(|o| o.parse::<u8>().is_ok()).count() != 4 {
        return None;
    }
    let dc = it.next()?.parse::<i32>().ok()?;
    if !valid_dc(dc) {
        return None;
    }
    let media = matches!(
        it.next().map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("media") | Some("1") | Some("true") | Some("m")
    );
    Some((ip.to_string(), dc, media))
}

/// Load the admin-editable and persisted-learned files into the in-memory map.
/// Call once at startup. The learned file wins over the manual file on conflict
/// (it reflects what was actually observed on the wire).
pub fn load() {
    let mut map = LEARNED.write().unwrap();
    let mut n = 0usize;
    for path in [manual_file(), learn_file()] {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            if let Some((ip, dc, media)) = parse_line(line) {
                map.insert(ip, (dc, media));
                n += 1;
            }
        }
    }
    if n > 0 {
        log::info!("dc_learn: loaded {n} IP->DC mapping(s)");
    }
}

/// Resolve a learned mapping for `ip`, if any.
pub fn lookup(ip: &str) -> Option<(i32, bool)> {
    LEARNED.read().unwrap().get(ip).copied()
}

/// Record `ip → (dc, is_media)` observed from a handshake that embedded the DC.
/// No-op when the IP is already resolvable from the hardcoded tables or was
/// already learned with the same DC — so the persist file only grows on a
/// genuinely new IP (flash-friendly).
pub fn learn(ip: &str, dc: i32, is_media: bool) {
    if ip.is_empty() || !valid_dc(dc) || dc_from_hardcoded(ip).is_some() {
        return;
    }
    {
        // Fast path: already known with the same DC — nothing to do.
        if let Some(&(d, _)) = LEARNED.read().unwrap().get(ip) {
            if d == dc {
                return;
            }
        }
    }
    let is_new = {
        let mut map = LEARNED.write().unwrap();
        match map.get(ip) {
            Some(&(d, _)) if d == dc => false,
            _ => {
                map.insert(ip.to_string(), (dc, is_media));
                true
            }
        }
    };
    if is_new {
        persist(ip, dc, is_media);
        log::info!("dc_learn: learned {ip} -> DC{dc} (media={is_media})");
    }
}

/// Append a single learned entry to the persist file (best-effort).
fn persist(ip: &str, dc: i32, is_media: bool) {
    let path = learn_file();
    let tag = if is_media { " media" } else { "" };
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{ip} {dc}{tag}") {
                log::warn!("dc_learn: append to {path} failed: {e}");
            }
        }
        Err(e) => log::warn!("dc_learn: open {path} failed: {e}"),
    }
}

/// Number of learned/loaded entries (for status/diagnostics).
pub fn len() -> usize {
    LEARNED.read().unwrap().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_variants() {
        assert_eq!(
            parse_line("149.154.167.35 2"),
            Some(("149.154.167.35".to_string(), 2, false))
        );
        assert_eq!(
            parse_line("149.154.171.255 5 media  # dc5 cdn"),
            Some(("149.154.171.255".to_string(), 5, true))
        );
        assert_eq!(parse_line("  # comment"), None);
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("garbage"), None);
        assert_eq!(parse_line("1.2.3.4 99"), None); // invalid dc
        assert_eq!(parse_line("not.an.ip 2"), None);
    }

    #[test]
    fn learn_and_lookup() {
        // Use an IP not in the hardcoded tables.
        let ip = "203.0.113.7";
        std::env::set_var("WRTG_DC_LEARN_FILE", "/dev/null");
        assert_eq!(lookup(ip), None);
        learn(ip, 4, false);
        assert_eq!(lookup(ip), Some((4, false)));
        // Re-learning the same DC is a no-op (doesn't panic/duplicate).
        learn(ip, 4, false);
        assert_eq!(lookup(ip), Some((4, false)));
        std::env::remove_var("WRTG_DC_LEARN_FILE");
    }

    #[test]
    fn hardcoded_ip_not_learned() {
        // 149.154.171.255 is in the hardcoded alt table -> learn() must skip it,
        // leaving no learned entry (hardcoded path already resolves it).
        std::env::set_var("WRTG_DC_LEARN_FILE", "/dev/null");
        learn("149.154.171.255", 5, true);
        assert_eq!(lookup("149.154.171.255"), None);
        std::env::remove_var("WRTG_DC_LEARN_FILE");
    }
}
