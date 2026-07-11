//! Per-DC WS blacklist with TTL (HTTP 302 avoidance).

use std::sync::OnceLock;
use std::time::Duration;

use crate::ttl_map::TtlMap;

static BLACKLIST: TtlMap<(i32, bool)> = TtlMap::new();

const DEFAULT_BLACKLIST_TTL_SEC: u64 = 45 * 60;

fn blacklist_ttl() -> Duration {
    #[cfg(test)]
    {
        if let Ok(s) = std::env::var("WRTG_WS_BLACKLIST_TTL_SEC") {
            if let Ok(secs) = s.parse::<u64>() {
                return Duration::from_secs(secs);
            }
        }
    }
    static TTL: OnceLock<Duration> = OnceLock::new();
    *TTL.get_or_init(|| {
        let secs = std::env::var("WRTG_WS_BLACKLIST_TTL_SEC")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_BLACKLIST_TTL_SEC);
        Duration::from_secs(secs)
    })
}

pub fn ws_blacklisted(dc: i32, is_media: bool) -> bool {
    BLACKLIST.is_active(&(dc, is_media))
}

pub fn mark_ws_blacklisted(dc: i32, is_media: bool) {
    let ttl_secs = blacklist_ttl().as_secs();
    BLACKLIST.mark((dc, is_media), blacklist_ttl());
    let media_tag = if is_media { " media" } else { "" };
    log::info!("DC{dc}{media_tag} WS blacklisted for {ttl_secs}s (HTTP 302 on all domains)");
}

#[cfg(test)]
fn reset_blacklist_for_test() {
    BLACKLIST.clear_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration as StdDuration;

    // Both tests mutate the global BLACKLIST; serialize them so one's reset()
    // can't wipe the other's entries mid-run.
    static TEST_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn blacklist_roundtrip() {
        let _g = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reset_blacklist_for_test();
        assert!(!ws_blacklisted(2, false));
        mark_ws_blacklisted(2, false);
        assert!(ws_blacklisted(2, false));
        assert!(!ws_blacklisted(2, true));
    }

    #[test]
    fn blacklist_ttl_expiry() {
        let _g = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reset_blacklist_for_test();
        std::env::set_var("WRTG_WS_BLACKLIST_TTL_SEC", "1");
        // Force re-read on next mark (OnceLock already set in prior tests — use fresh DC).
        mark_ws_blacklisted(99, false);
        assert!(ws_blacklisted(99, false));
        thread::sleep(StdDuration::from_millis(1100));
        assert!(!ws_blacklisted(99, false));
        assert!(!BLACKLIST.contains(&(99, false)));
    }
}
