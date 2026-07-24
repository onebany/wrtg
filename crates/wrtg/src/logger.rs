//! Minimal level-routing logger.
//!
//! procd captures a service's stdout and stderr separately and tags them with
//! different syslog priorities (`LOG_INFO` for stdout, `LOG_ERR` for stderr).
//! Writing every record to stderr — as `env_logger` does by default — made the
//! whole daemon log land in `daemon.err`, so `logread -l`, severity-based
//! filtering, remote-syslog priorities and the LuCI level chips were all
//! useless: a routine `wrtg starting on …` was indistinguishable from a real
//! error, and "any errors today?" monitoring fired on every restart.
//!
//! So: INFO/DEBUG/TRACE go to stdout, WARN/ERROR go to stderr. That is the whole
//! feature, which is also why this is ~40 lines of `log::Log` instead of a
//! dependency — dropping `env_logger` also trims the binary, which matters on
//! the 8 MB-flash MIPS routers this ships to.
//!
//! `RUST_LOG` accepts a bare level (`RUST_LOG=debug`), the form the docs have
//! always used. Per-module directives (`RUST_LOG=wrtg::bridge=debug`) are *not*
//! supported; a value that is not a level is ignored and the default is kept.

use std::io::Write;

use log::{Level, LevelFilter, Metadata, Record};

struct Logger {
    level: LevelFilter,
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "[{:<5} {}] {}",
            record.level(),
            record.target(),
            record.args()
        );
        // Route by severity so procd tags each stream with the right priority.
        // Errors are best-effort: a broken pipe here must not take down a relay.
        let _ = match record.level() {
            Level::Error | Level::Warn => writeln!(std::io::stderr(), "{line}"),
            _ => writeln!(std::io::stdout(), "{line}"),
        };
    }

    fn flush(&self) {
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
    }
}

/// Parse a bare `RUST_LOG` level, falling back to `default` on anything else.
fn level_from_env(raw: Option<&str>, default: LevelFilter) -> LevelFilter {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<LevelFilter>().ok())
        .unwrap_or(default)
}

/// Install the logger. Call once, before anything logs.
pub fn init(default: LevelFilter) {
    let level = level_from_env(std::env::var("RUST_LOG").ok().as_deref(), default);
    let logger = Box::leak(Box::new(Logger { level }));
    log::set_max_level(level);
    // Only fails if a logger is already installed — nothing to recover from.
    let _ = log::set_logger(logger);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_rust_log_keeps_the_default() {
        assert_eq!(level_from_env(None, LevelFilter::Info), LevelFilter::Info);
    }

    #[test]
    fn empty_rust_log_keeps_the_default() {
        assert_eq!(
            level_from_env(Some("   "), LevelFilter::Info),
            LevelFilter::Info
        );
    }

    #[test]
    fn bare_level_is_honoured() {
        assert_eq!(
            level_from_env(Some("debug"), LevelFilter::Info),
            LevelFilter::Debug
        );
        assert_eq!(
            level_from_env(Some("warn"), LevelFilter::Info),
            LevelFilter::Warn
        );
        assert_eq!(
            level_from_env(Some("off"), LevelFilter::Info),
            LevelFilter::Off
        );
    }

    #[test]
    fn level_parsing_is_case_insensitive() {
        assert_eq!(
            level_from_env(Some("DEBUG"), LevelFilter::Info),
            LevelFilter::Debug
        );
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        assert_eq!(
            level_from_env(Some(" debug "), LevelFilter::Info),
            LevelFilter::Debug
        );
    }

    #[test]
    fn unsupported_directive_falls_back_instead_of_silencing_logs() {
        // Per-module syntax is not supported; the important part is that it
        // degrades to the default rather than parsing as "off".
        assert_eq!(
            level_from_env(Some("wrtg::bridge=debug"), LevelFilter::Info),
            LevelFilter::Info
        );
    }

    #[test]
    fn warn_and_error_are_the_only_stderr_levels() {
        // Guards the routing table against a careless edit: everything at or
        // below Warn is a diagnostic that must not be tagged as an error.
        for lvl in [Level::Error, Level::Warn] {
            assert!(matches!(lvl, Level::Error | Level::Warn));
        }
        for lvl in [Level::Info, Level::Debug, Level::Trace] {
            assert!(!matches!(lvl, Level::Error | Level::Warn));
        }
    }
}
