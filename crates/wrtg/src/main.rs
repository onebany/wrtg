use std::env;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::net::TcpStream;

use wrtg::bridge::{
    blind_relay, is_self_target, should_skip_ws, try_cf_fallback, try_tcp_fallback, try_ws_bridge,
    CfBridgeResult, TcpFallbackResult, WsBridgeResult,
};
use wrtg::cf_balancer::proxy_domains;
use wrtg::cf_proxy_domains::{
    cfproxy_auto_enabled, seed_default_cfproxy_domains, start_cfproxy_refresh_task,
};
use wrtg::cf_worker_pool::{start_refill_task as start_cf_refill, warmup_pools as warmup_cf_pools};
use wrtg::config::{apply_config, load_from_env};
use wrtg::handshake::read_client_init;
use wrtg::mtproto::{
    dc_from_orig_dst, generate_relay_init, proto_tag_for, ws_redirect_blacklist_warranted,
    ws_target_ip, CryptoCtx, HandshakeInfo,
};
use wrtg::sockopt::{get_original_dst, tune_tcp};
use wrtg::watchdog::{bind_transparent, serve};
use wrtg::ws_blacklist::mark_ws_blacklisted;
use wrtg::ws_pool::{start_refill_task, warmup_pools};

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // A hand-run `--check` lacks the procd environment the daemon is started
    // with, so it would report the config file's worker/proxy as "not set".
    // Seed env from the config file first (real env still wins) so diagnostics
    // reflect the running setup.
    let check_mode = env::args()
        .skip(1)
        .any(|a| a.trim_start_matches('-').trim_end_matches('\r') == "check");
    if check_mode {
        for (k, v) in wrtg::config::import_config_file(&wrtg::config::config_file_path()) {
            if env::var_os(&k).is_none() {
                env::set_var(&k, v);
            }
        }
    }

    let mut cfg = load_from_env();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let key = arg.trim_start_matches('-').trim_end_matches('\r');
        match key {
            "check" => {}
            "listen" => {
                if let Some(v) = args.next() {
                    cfg.listen_addr = v.trim_end_matches('\r').to_string();
                }
            }
            "front-ip" => {
                if let Some(v) = args.next() {
                    cfg.front_ip = v.trim_end_matches('\r').to_string();
                }
            }
            "help" | "h" => {
                eprintln!("usage: wrtg [--listen ADDR] [--front-ip IP] [--check] [--version]");
                return;
            }
            "version" => {
                println!("wrtg {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    apply_config(&cfg);

    if check_mode {
        std::process::exit(wrtg::check::run_check(&cfg).await);
    }
    wrtg::dc_learn::load();

    if cfproxy_auto_enabled() {
        seed_default_cfproxy_domains();
        start_cfproxy_refresh_task();
    }

    start_refill_task();
    start_cf_refill();
    warmup_pools();
    warmup_cf_pools();

    log::info!(
        "wrtg starting on {} (front-ip={} front-dcs={:?}, cf-workers={}, cf-proxies={})",
        cfg.listen_addr,
        cfg.front_ip,
        cfg.front_dcs,
        cfg.cf_worker_domains.len(),
        proxy_domains().len()
    );

    spawn_reload_handler();

    let listener = match bind_transparent(&cfg.listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("listen: {e}");
            std::process::exit(1);
        }
    };

    serve(listener, cfg.listen_addr.clone(), handle_conn).await;
}

/// Reload front/domains + DC-learn from the config file on SIGHUP, so config
/// edits apply without dropping live sessions. LISTEN / nftables changes still
/// need a restart (the listener is already bound).
fn spawn_reload_handler() {
    tokio::spawn(async {
        use tokio::signal::unix::{signal, SignalKind};
        let mut hup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("SIGHUP handler unavailable: {e}");
                return;
            }
        };
        while hup.recv().await.is_some() {
            log::info!("SIGHUP received — reloading config");
            wrtg::config::reload_from_file();
        }
    });
}

async fn handle_conn(stream: TcpStream) {
    tune_tcp(&stream);
    let remote = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_default();

    let (orig_ip, orig_port) = match get_original_dst(&stream) {
        Ok((ip, port)) => (ip, port),
        Err(e) => {
            log::debug!("original dst: {e}");
            (String::new(), 0)
        }
    };

    let label = if orig_ip.is_empty() {
        remote.clone()
    } else {
        format!("{remote} -> {orig_ip}:{orig_port}")
    };

    // A direct connect to the listener (no DNAT) makes SO_ORIGINAL_DST return
    // the listener's own address. Relaying that would connect the daemon to
    // itself and recurse until the connection semaphore is exhausted — drop it.
    if is_self_target(stream.local_addr().ok(), &orig_ip, orig_port) {
        static SELF_CONNECT_WARNED: AtomicBool = AtomicBool::new(false);
        if !SELF_CONNECT_WARNED.swap(true, Ordering::Relaxed) {
            log::warn!("[{label}] original dst is the listener itself; dropping self-connect");
        }
        return;
    }

    match read_client_init(stream).await {
        Ok(Some(parsed)) => {
            handle_handshake(parsed.info, parsed.stream, &orig_ip, orig_port, &label).await;
        }
        Ok(None) => {}
        Err((stream, raw, err)) => {
            if err == "tls passthrough" {
                // Every TLS passthrough connection lands here; a WARN per
                // connection floods the router syslog.
                log::debug!(
                    "[{label}] init: tls passthrough len={} -> passthrough",
                    raw.len()
                );
            } else {
                let head = if raw.len() > 64 { &raw[..64] } else { &raw };
                log::warn!(
                    "[{label}] init: {err} len={} raw64={head:02x?} -> passthrough",
                    raw.len()
                );
            }
            blind_relay(stream, &orig_ip, orig_port, &raw, &label).await;
        }
    }
}

async fn handle_handshake(
    mut hs: HandshakeInfo,
    client: wrtg::handshake::PrefixedStream,
    orig_ip: &str,
    orig_port: u16,
    label: &str,
) {
    if hs.dc_in_pkt && !orig_ip.is_empty() {
        // The client told us the DC in-band — remember which IP that was, so a
        // later client that omits it can still be resolved to the fast front.
        wrtg::dc_learn::learn(orig_ip, hs.dc, hs.is_media);
    } else if !orig_ip.is_empty() {
        if let Some((dc, media)) = dc_from_orig_dst(orig_ip) {
            hs.dc = dc;
            hs.is_media = media;
            log::info!("[{label}] DC{dc} from orig dst {orig_ip} (media={media})");
        }
    }

    if hs.dc == 0 {
        log::warn!("[{label}] handshake OK but DC unknown (orig={orig_ip}) -> passthrough");
        let (stream, extra) = client.into_parts();
        let mut raw = hs.handshake.to_vec();
        raw.extend_from_slice(&extra);
        blind_relay(stream, orig_ip, orig_port, &raw, label).await;
        return;
    }

    let media_tag = if hs.is_media { " media" } else { "" };
    log::info!(
        "[{label}] direct handshake OK: DC{}{media_tag} proto=0x{:08X}",
        hs.dc,
        hs.proto_int
    );

    let dc_idx = if hs.is_media { -hs.dc } else { hs.dc } as i16;
    let proto_tag = proto_tag_for(hs.proto_int);

    let relay_init = match generate_relay_init(proto_tag, dc_idx) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[{label}] relay init: {e}");
            blind_relay_prefixed(client, &hs, orig_ip, orig_port, label).await;
            return;
        }
    };

    let ctx = match CryptoCtx::build_direct(&hs.handshake, &relay_init) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[{label}] crypto ctx: {e}");
            blind_relay_prefixed(client, &hs, orig_ip, orig_port, label).await;
            return;
        }
    };

    let bridge_media = hs.is_media;
    let mut client = client;
    let mut ctx = ctx;

    if should_skip_ws(hs.dc, bridge_media, orig_ip) {
        log::info!(
            "[{label}] DC{}{media_tag} WS skipped (blacklist or DC ip_fail)",
            hs.dc
        );
    } else {
        match try_ws_bridge(client, &hs, &relay_init, ctx, orig_ip, label).await {
            WsBridgeResult::Connected => return,
            WsBridgeResult::Failed {
                client: c,
                ctx: cx,
                all_blocked,
                timed_out: _,
            } => {
                client = c;
                ctx = cx;
                if all_blocked {
                    let target = ws_target_ip(hs.dc, orig_ip);
                    if ws_redirect_blacklist_warranted(hs.dc, &target) {
                        mark_ws_blacklisted(hs.dc, bridge_media);
                    }
                }
            }
        }
    }

    match try_cf_fallback(client, &hs, &relay_init, ctx, orig_ip, label).await {
        CfBridgeResult::Connected => return,
        CfBridgeResult::Failed { client: c, ctx: cx } => {
            client = c;
            ctx = cx;
        }
    }

    match try_tcp_fallback(
        client,
        &relay_init,
        ctx,
        orig_ip,
        hs.dc,
        bridge_media,
        label,
    )
    .await
    {
        TcpFallbackResult::Connected => return,
        TcpFallbackResult::Failed(c) => client = c,
    };

    log::warn!("[{label}] all bridge paths failed, blind relay");
    let (stream, extra) = client.into_parts();
    let mut initial = hs.handshake.to_vec();
    initial.extend_from_slice(&extra);
    blind_relay(stream, orig_ip, orig_port, &initial, label).await;
}

/// Blind-relay fallback that replays the already-read client handshake (plus
/// any bytes past it) upstream — dropping them would leave the session hung.
async fn blind_relay_prefixed(
    client: wrtg::handshake::PrefixedStream,
    hs: &HandshakeInfo,
    orig_ip: &str,
    orig_port: u16,
    label: &str,
) {
    let (stream, extra) = client.into_parts();
    let mut raw = hs.handshake.to_vec();
    raw.extend_from_slice(&extra);
    blind_relay(stream, orig_ip, orig_port, &raw, label).await;
}
