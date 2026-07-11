use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::cf_balancer::{
    cf_fallback_disabled, proxy_domains_for_dc, update_proxy_domain_for_dc, worker_domains,
    worker_domains_for_dc, worker_passthrough_disabled,
};
use crate::cf_proxy::try_cf_proxy_domain;
use crate::cf_worker_pool::{acquire as acquire_cf_worker, schedule_refill as schedule_cf_refill};
use crate::fronting::{
    clear_fronting_fail, mark_fronting_failed, should_skip_fronting, try_ws_fronting,
};
use crate::handshake::PrefixedStream;
use crate::ip_fail::{
    clear_dc_fail, clear_ip_fail, mark_dc_failed, mark_ip_failed, should_skip_direct_ws,
    ws_connect_timeout,
};
use crate::media::{
    http_front_host, http_front_passthrough_payload, should_try_worker_passthrough,
};
use crate::mtproto::{
    dc_default_ip, tcp_fallback_targets, ws_domains, ws_target_ip, CryptoCtx, HandshakeInfo,
};
use crate::sockopt::{tune_tcp, RELAY_BUF_SIZE};
use crate::splitter::MsgSplitter;
use crate::tls_sni::{passthrough_host, passthrough_targets};
use crate::ws::{
    connect_cf_worker_tcp, connect_cf_worker_ws, connect_ws, is_ws_redirect, ws_ping_frame,
    RawWebSocket,
};
use crate::ws_blacklist::ws_blacklisted;
use crate::ws_pool::{acquire, schedule_refill};

const WS_CHANNEL_CAP: usize = 256;
const WS_SEND_BATCH_MAX: usize = 32;
const CF_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_CF_PROXY_ATTEMPTS: usize = 3;

fn ws_ping_interval() -> Duration {
    static D: std::sync::LazyLock<Duration> = std::sync::LazyLock::new(|| {
        std::env::var("WRTG_WS_PING_SEC")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(30))
    });
    *D
}

pub async fn bridge_ws(
    client: PrefixedStream,
    ws: RawWebSocket,
    ctx: CryptoCtx,
    splitter: Option<MsgSplitter>,
    label: &str,
    dc: i32,
    is_media: bool,
) {
    let (mut client_read, mut client_write) = tokio::io::split(client);
    let (mut up_crypto, mut down_crypto) = ctx.split();
    let label_down = label.to_string();
    let dc_tag = fmt_dc(dc, is_media);
    let media_tag = if is_media { " media" } else { "" };

    let (mut ws_read, mut ws_write) = ws.into_halves();
    let (send_tx, mut send_rx) = mpsc::channel::<Vec<u8>>(WS_CHANNEL_CAP);
    let (recv_tx, mut recv_rx) = mpsc::channel::<Vec<u8>>(WS_CHANNEL_CAP);
    let (pong_tx, mut pong_rx) = mpsc::channel::<Vec<u8>>(8);

    let pong_tx_ping = pong_tx.clone();
    let ping_driver = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(ws_ping_interval());
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if pong_tx_ping.send(ws_ping_frame()).await.is_err() {
                break;
            }
        }
    });

    let ws_send_driver = tokio::spawn(async move {
        loop {
            tokio::select! {
                data = send_rx.recv() => {
                    match data {
                        Some(first) => {
                            let mut batch = vec![first];
                            while batch.len() < WS_SEND_BATCH_MAX {
                                match send_rx.try_recv() {
                                    Ok(more) => batch.push(more),
                                    Err(_) => break,
                                }
                            }
                            if batch.len() == 1 {
                                if ws_write.send_binary(&batch[0]).await.is_err() {
                                    break;
                                }
                            } else if ws_write.send_batch(&batch).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                pong = pong_rx.recv() => {
                    match pong {
                        Some(frame) => {
                            if ws_write.send_raw(&frame).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });

    let pong_tx_recv = pong_tx.clone();
    let ws_recv_driver = tokio::spawn(async move {
        while let Ok(Some(p)) = ws_read.recv_binary(&pong_tx_recv).await {
            if recv_tx.send(p).await.is_err() {
                break;
            }
        }
    });

    let send_tx_up = send_tx.clone();
    let mut splitter = splitter;
    let mut up = tokio::spawn(async move {
        let mut buf = vec![0u8; RELAY_BUF_SIZE];
        loop {
            match client_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = up_crypto.client_to_telegram(&buf[..n]);
                    if let Some(ref mut sp) = splitter {
                        let parts = sp.split(&chunk);
                        if parts.is_empty() {
                            continue;
                        }
                        for part in parts {
                            if send_tx_up.send(part).await.is_err() {
                                return;
                            }
                        }
                    } else if send_tx_up.send(chunk).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        if let Some(ref mut sp) = splitter {
            for tail in sp.flush() {
                let _ = send_tx_up.send(tail).await;
            }
        }
    });

    let mut down = tokio::spawn(async move {
        while let Some(payload) = recv_rx.recv().await {
            let out = down_crypto.telegram_to_client(&payload);
            if client_write.write_all(&out).await.is_err() {
                break;
            }
        }
    });

    let up_finished = tokio::select! {
        _ = &mut up => {
            down.abort();
            true
        },
        _ = &mut down => {
            up.abort();
            false
        },
    };
    drop(send_tx);
    drop(pong_tx);
    ping_driver.abort();
    ws_send_driver.abort();
    ws_recv_driver.abort();
    if up_finished {
        let _ = down.await;
    } else {
        let _ = up.await;
    }
    let _ = ws_send_driver.await;
    let _ = ws_recv_driver.await;
    let _ = ping_driver.await;
    log::info!("[{label_down}] {dc_tag} WS session closed{media_tag}");
}

pub async fn bridge_tcp(client: PrefixedStream, remote: TcpStream, ctx: CryptoCtx, label: &str) {
    let (mut cr, mut cw) = tokio::io::split(client);
    let (mut rr, mut rw) = remote.into_split();
    let (mut up_crypto, mut down_crypto) = ctx.split();

    let mut up = tokio::spawn(async move {
        let mut buf = vec![0u8; RELAY_BUF_SIZE];
        loop {
            match cr.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let out = up_crypto.client_to_telegram(&buf[..n]);
                    if rw.write_all(&out).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = rw.shutdown().await;
    });

    let mut down = tokio::spawn(async move {
        let mut buf = vec![0u8; RELAY_BUF_SIZE];
        loop {
            match rr.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let out = down_crypto.telegram_to_client(&buf[..n]);
                    if cw.write_all(&out).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = cw.shutdown().await;
    });

    let up_finished = tokio::select! {
        _ = &mut up => {
            down.abort();
            true
        },
        _ = &mut down => {
            up.abort();
            false
        },
    };
    if up_finished {
        let _ = down.await;
    } else {
        let _ = up.await;
    }
    log::info!("[{label}] TCP fallback session closed");
}

#[allow(clippy::large_enum_variant)]
pub enum WsBridgeResult {
    Connected,
    Failed {
        client: PrefixedStream,
        ctx: CryptoCtx,
        all_blocked: bool,
        timed_out: bool,
    },
}

async fn try_ws_with_domains(
    target_ip: &str,
    domains: &[String],
    relay_init: &[u8],
    label: &str,
    dc: i32,
    is_media: bool,
) -> Result<(RawWebSocket, String), (bool, bool)> {
    let mut all_blocked = true;
    let mut timed_out = false;
    let connect_timeout = ws_connect_timeout(dc, is_media);

    for domain in domains {
        log::info!("[{label}] DC{dc} -> trying WSS {domain} via {target_ip}");
        match timeout(
            connect_timeout,
            connect_ws(target_ip, domain, "/apiws", connect_timeout),
        )
        .await
        {
            Err(_) => {
                log::warn!("[{label}] DC{dc} WS {domain} timeout");
                all_blocked = false;
                timed_out = true;
                continue;
            }
            Ok(Err(e)) => {
                log::warn!("[{label}] DC{dc} WS {domain} failed: {e}");
                if !is_ws_redirect(&e) {
                    all_blocked = false;
                }
                if e.kind() == std::io::ErrorKind::TimedOut {
                    timed_out = true;
                }
                continue;
            }
            Ok(Ok(mut ws)) => {
                if let Err(e) = ws.send(relay_init).await {
                    ws.close().await;
                    log::warn!("[{label}] DC{dc} relay init send failed: {e}");
                    all_blocked = false;
                    continue;
                }
                return Ok((ws, domain.clone()));
            }
        }
    }
    Err((all_blocked, timed_out))
}

pub async fn try_ws_bridge(
    client: PrefixedStream,
    hs: &HandshakeInfo,
    relay_init: &[u8],
    ctx: CryptoCtx,
    orig_ip: &str,
    label: &str,
) -> WsBridgeResult {
    let target_ip = ws_target_ip(hs.dc, orig_ip);
    if target_ip.is_empty() {
        return WsBridgeResult::Failed {
            client,
            ctx,
            all_blocked: false,
            timed_out: false,
        };
    }

    let splitter = MsgSplitter::new(relay_init, hs.proto_int).ok();

    if !hs.is_media {
        if let Some(mut pooled) = acquire(hs.dc, hs.is_media).await {
            match pooled.ws.send(relay_init).await {
                Ok(()) => {
                    clear_ip_fail(&target_ip, hs.dc);
                    clear_dc_fail(hs.dc, hs.is_media);
                    clear_fronting_fail(&target_ip, hs.dc);
                    log::info!(
                        "[{label}] DC{} -> WS connected via pool ({})",
                        hs.dc,
                        pooled.domain
                    );
                    bridge_ws(client, pooled.ws, ctx, splitter, label, hs.dc, hs.is_media).await;
                    schedule_refill(hs.dc, hs.is_media, target_ip.clone());
                    return WsBridgeResult::Connected;
                }
                Err(e) => {
                    pooled.ws.close().await;
                    log::warn!("[{label}] DC{} pooled WS relay init failed: {e}", hs.dc);
                }
            }
        }
    }

    let domains = ws_domains(hs.dc, hs.is_media);
    let connect_timeout = ws_connect_timeout(hs.dc, hs.is_media);
    let (all_blocked, timed_out) = match try_ws_with_domains(
        &target_ip,
        &domains,
        relay_init,
        label,
        hs.dc,
        hs.is_media,
    )
    .await
    {
        Ok((ws, connected_domain)) => {
            clear_ip_fail(&target_ip, hs.dc);
            clear_dc_fail(hs.dc, hs.is_media);
            clear_fronting_fail(&target_ip, hs.dc);
            log::info!(
                "[{label}] DC{} -> WS connected via {}",
                hs.dc,
                connected_domain
            );
            bridge_ws(client, ws, ctx, splitter, label, hs.dc, hs.is_media).await;
            schedule_refill(hs.dc, hs.is_media, target_ip);
            return WsBridgeResult::Connected;
        }
        Err((blocked, to)) => (blocked, to),
    };

    if !should_skip_fronting(&target_ip, hs.dc) {
        match try_ws_fronting(
            &target_ip,
            hs.dc,
            hs.is_media,
            relay_init,
            label,
            connect_timeout,
        )
        .await
        {
            Ok((ws, connected_domain)) => {
                clear_ip_fail(&target_ip, hs.dc);
                clear_dc_fail(hs.dc, hs.is_media);
                clear_fronting_fail(&target_ip, hs.dc);
                log::info!(
                    "[{label}] DC{} -> WS connected via fronting {}",
                    hs.dc,
                    connected_domain
                );
                bridge_ws(client, ws, ctx, splitter, label, hs.dc, hs.is_media).await;
                schedule_refill(hs.dc, hs.is_media, target_ip);
                return WsBridgeResult::Connected;
            }
            Err((f_blocked, f_timed_out)) => {
                mark_fronting_failed(&target_ip, hs.dc);
                if !f_blocked {
                    // all_blocked stays as from direct attempt
                }
                if f_timed_out {
                    // timed_out may already be true from direct
                }
            }
        }
    }

    if timed_out {
        mark_ip_failed(&target_ip, hs.dc);
    }
    mark_dc_failed(hs.dc, hs.is_media);

    WsBridgeResult::Failed {
        client,
        ctx,
        all_blocked,
        timed_out,
    }
}

#[allow(clippy::large_enum_variant)]
pub enum CfBridgeResult {
    Connected,
    Failed {
        client: PrefixedStream,
        ctx: CryptoCtx,
    },
}

pub async fn try_cf_fallback(
    client: PrefixedStream,
    hs: &HandshakeInfo,
    relay_init: &[u8],
    ctx: CryptoCtx,
    orig_ip: &str,
    label: &str,
) -> CfBridgeResult {
    if cf_fallback_disabled() {
        return CfBridgeResult::Failed { client, ctx };
    }

    let splitter = MsgSplitter::new(relay_init, hs.proto_int).ok();
    let dst = ws_target_ip(hs.dc, orig_ip);
    let dst_fallback = if dst.is_empty() {
        dc_default_ip(hs.dc)
            .unwrap_or("149.154.167.220")
            .to_string()
    } else {
        dst.clone()
    };

    // CF Worker pool
    if let Some(mut pooled) = acquire_cf_worker(hs.dc, hs.is_media, orig_ip).await {
        match pooled.ws.send(relay_init).await {
            Ok(()) => {
                log::info!(
                    "[{label}] DC{} -> WS connected via CF worker pool ({})",
                    hs.dc,
                    pooled.worker
                );
                bridge_ws(client, pooled.ws, ctx, splitter, label, hs.dc, hs.is_media).await;
                schedule_cf_refill(hs.dc, hs.is_media, orig_ip.to_string());
                return CfBridgeResult::Connected;
            }
            Err(e) => {
                pooled.ws.close().await;
                log::warn!(
                    "[{label}] DC{} CF worker pool relay init failed: {e}",
                    hs.dc
                );
            }
        }
    }

    // CF Worker direct
    for worker in worker_domains_for_dc(hs.dc) {
        log::info!("[{label}] DC{} -> trying CF worker {worker}", hs.dc);
        match timeout(
            CF_CONNECT_TIMEOUT,
            connect_cf_worker_ws(&worker, &dst_fallback, hs.dc, CF_CONNECT_TIMEOUT),
        )
        .await
        {
            Ok(Ok(mut ws)) => {
                if let Err(e) = ws.send(relay_init).await {
                    ws.close().await;
                    log::warn!("[{label}] DC{} CF worker relay init failed: {e}", hs.dc);
                    continue;
                }
                log::info!(
                    "[{label}] DC{} -> WS connected via CF worker {worker}",
                    hs.dc
                );
                bridge_ws(client, ws, ctx, splitter, label, hs.dc, hs.is_media).await;
                schedule_cf_refill(hs.dc, hs.is_media, orig_ip.to_string());
                return CfBridgeResult::Connected;
            }
            Ok(Err(e)) => {
                log::warn!("[{label}] DC{} CF worker {worker} failed: {e}", hs.dc);
            }
            Err(_) => {
                log::warn!("[{label}] DC{} CF worker {worker} timeout", hs.dc);
            }
        }
    }

    // CF Proxy balancer: primary sequential, then parallel race
    let cf_domains: Vec<_> = proxy_domains_for_dc(hs.dc)
        .into_iter()
        .take(MAX_CF_PROXY_ATTEMPTS)
        .collect();

    if let Some(primary) = cf_domains.first() {
        log::info!("[{label}] DC{} -> trying CF proxy {primary}", hs.dc);
        if let Some((ws, chosen)) =
            finish_cf_proxy_connect(primary, hs.dc, hs.is_media, relay_init, label).await
        {
            update_proxy_domain_for_dc(hs.dc, &chosen);
            bridge_ws(client, ws, ctx, splitter, label, hs.dc, hs.is_media).await;
            return CfBridgeResult::Connected;
        }
    }

    if cf_domains.len() > 1 {
        let dc = hs.dc;
        let is_media = hs.is_media;
        let label_owned = label.to_string();
        let relay_init_owned = relay_init.to_vec();
        let mut handles = Vec::new();
        for cf_domain in cf_domains.into_iter().skip(1) {
            let label_spawn = label_owned.clone();
            let relay = relay_init_owned.clone();
            handles.push(tokio::spawn(async move {
                finish_cf_proxy_connect(&cf_domain, dc, is_media, &relay, &label_spawn).await
            }));
        }
        for h in handles {
            if let Ok(Some((ws, chosen))) = h.await {
                update_proxy_domain_for_dc(hs.dc, &chosen);
                bridge_ws(client, ws, ctx, splitter, label, hs.dc, hs.is_media).await;
                return CfBridgeResult::Connected;
            }
        }
    }

    CfBridgeResult::Failed { client, ctx }
}

async fn finish_cf_proxy_connect(
    cf_domain: &str,
    dc: i32,
    is_media: bool,
    relay_init: &[u8],
    label: &str,
) -> Option<(RawWebSocket, String)> {
    let cf_domain_owned = cf_domain.to_string();
    match timeout(
        CF_CONNECT_TIMEOUT,
        try_cf_proxy_domain(cf_domain, dc, is_media, CF_CONNECT_TIMEOUT),
    )
    .await
    {
        Ok(Ok(mut ws)) => {
            if let Err(e) = ws.send(relay_init).await {
                ws.close().await;
                log::warn!("[{label}] DC{dc} CF proxy relay init failed: {e}");
                return None;
            }
            log::info!("[{label}] DC{dc} -> WS connected via CF proxy {cf_domain_owned}");
            Some((ws, cf_domain_owned))
        }
        Ok(Err(e)) => {
            if matches!(
                e,
                crate::ws::WsConnectError::Io(ref io)
                    if io.kind() == std::io::ErrorKind::WouldBlock
            ) {
                log::debug!(
                    "[{label}] DC{dc} CF proxy {cf_domain_owned} skipped: {}",
                    e.into_io()
                );
            } else {
                log::warn!(
                    "[{label}] DC{dc} CF proxy {cf_domain_owned} failed: {}",
                    e.into_io()
                );
            }
            None
        }
        Err(_) => {
            log::warn!("[{label}] DC{dc} CF proxy {cf_domain_owned} timeout");
            None
        }
    }
}

pub enum TcpFallbackResult {
    Connected,
    Failed(PrefixedStream),
}

pub async fn try_tcp_fallback(
    client: PrefixedStream,
    relay_init: &[u8],
    ctx: CryptoCtx,
    orig_ip: &str,
    dc: i32,
    is_media: bool,
    label: &str,
) -> TcpFallbackResult {
    let blocked_cdn = crate::media::is_blocked_media_cdn(orig_ip);
    let blacklisted = ws_blacklisted(dc, is_media);
    let targets = tcp_fallback_targets(dc, orig_ip, is_media, blocked_cdn, blacklisted);

    for dst in targets {
        if dst.is_empty() {
            continue;
        }
        let addr = format!("{dst}:443");
        match timeout(Duration::from_secs(10), TcpStream::connect(&addr)).await {
            Err(_) | Ok(Err(_)) => {
                log::warn!("[{label}] TCP fallback to {dst} failed");
                continue;
            }
            Ok(Ok(mut remote)) => {
                tune_tcp(&remote);
                if remote.write_all(relay_init).await.is_err() {
                    continue;
                }
                log::info!("[{label}] DC{dc} -> TCP fallback to {dst}:443");
                bridge_tcp(client, remote, ctx, label).await;
                return TcpFallbackResult::Connected;
            }
        }
    }
    TcpFallbackResult::Failed(client)
}

/// Raw byte tunnel between the client and a CF Worker WS (no MTProto crypto).
/// Used to passthrough TLS / MTProto-over-HTTP media to a blocked DC's real IP.
async fn relay_via_worker(
    client: TcpStream,
    ws: RawWebSocket,
    initial: &[u8],
    label: &str,
    worker: &str,
    orig_ip: &str,
    port: u16,
) -> Result<(), TcpStream> {
    let mut ws = ws;
    if !initial.is_empty() {
        if let Err(e) = ws.send(initial).await {
            ws.close().await;
            log::warn!("[{label}] worker passthrough initial send failed: {e}");
            return Err(client);
        }
    }
    let http_host = if port == 80 {
        crate::tls_sni::parse_http_host(initial)
            .or_else(|| {
                let lower = String::from_utf8_lossy(initial).to_lowercase();
                let i = lower.find("\r\nhost:")?;
                let rest = &initial[i + 7..];
                let end = rest.iter().position(|&b| b == b'\r' || b == b'\n')?;
                let h = String::from_utf8_lossy(&rest[..end]).trim().to_string();
                if h.is_empty() {
                    None
                } else {
                    Some(h)
                }
            })
            .map(|h| format!(" host={h}"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    log::info!(
        "[{label}] passthrough via CF worker {worker} -> {orig_ip}:{port}{http_host} ({} bytes)",
        initial.len()
    );

    let (mut ws_read, mut ws_write) = ws.into_halves();
    let (mut cr, mut cw) = client.into_split();
    let (data_tx, mut data_rx) = mpsc::channel::<Vec<u8>>(WS_CHANNEL_CAP);
    let (pong_tx, mut pong_rx) = mpsc::channel::<Vec<u8>>(8);

    let writer = tokio::spawn(async move {
        loop {
            tokio::select! {
                d = data_rx.recv() => match d {
                    Some(b) => { if ws_write.send_binary(&b).await.is_err() { break; } }
                    None => break,
                },
                p = pong_rx.recv() => match p {
                    Some(f) => { if ws_write.send_raw(&f).await.is_err() { break; } }
                    None => break,
                },
            }
        }
    });

    let mut up = tokio::spawn(async move {
        let mut buf = vec![0u8; RELAY_BUF_SIZE];
        loop {
            match cr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if data_tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut down = tokio::spawn(async move {
        while let Ok(Some(payload)) = ws_read.recv_binary(&pong_tx).await {
            if cw.write_all(&payload).await.is_err() {
                break;
            }
        }
        let _ = cw.shutdown().await;
    });

    // Tear down as soon as either direction ends (client closed OR upstream closed),
    // so a stalled tunnel can't leak the connection.
    tokio::select! {
        _ = &mut up => down.abort(),
        _ = &mut down => up.abort(),
    }
    let _ = writer.await;
    log::info!("[{label}] worker passthrough session closed");
    Ok(())
}

/// Try to passthrough via the CF Worker (raw tunnel to `orig_ip:orig_port`).
/// Returns the client back on `Err` so the caller can fall back to the front.
///
/// Tries **every** configured worker (same as the MTProto CF path). The previous
/// first-only attempt silently fell through to front passthrough whenever the
/// primary worker was unreachable — with failures only at `debug`, so `logread`
/// never showed why media/emoji kept going to `FRONT_IP`.
async fn try_worker_passthrough(
    client: TcpStream,
    orig_ip: &str,
    orig_port: u16,
    initial: &[u8],
    label: &str,
) -> Result<(), TcpStream> {
    let mut client = client;
    if orig_ip.is_empty() {
        return Err(client);
    }
    if cf_fallback_disabled() {
        log::warn!("[{label}] worker passthrough skipped (WRTG_NO_CFPROXY)");
        return Err(client);
    }
    if worker_passthrough_disabled() {
        log::warn!("[{label}] worker passthrough skipped (WRTG_NO_WORKER_PASSTHROUGH)");
        return Err(client);
    }
    let workers = worker_domains();
    if workers.is_empty() {
        log::warn!("[{label}] worker passthrough skipped (no CF_WORKER_DOMAIN)");
        return Err(client);
    }
    let port = if orig_port == 0 { 443 } else { orig_port };
    // Tunnel straight to the client's real DC IP from the CF edge; the ISP
    // blocks it locally but the Worker reaches Telegram subnets.
    let dst_ip = orig_ip;
    log::info!(
        "[{label}] worker passthrough trying {} worker(s) -> {dst_ip}:{port}",
        workers.len()
    );
    for worker in &workers {
        match timeout(
            CF_CONNECT_TIMEOUT,
            connect_cf_worker_tcp(worker, dst_ip, port, CF_CONNECT_TIMEOUT),
        )
        .await
        {
            Ok(Ok(ws)) => {
                match relay_via_worker(client, ws, initial, label, worker, dst_ip, port).await {
                    Ok(()) => return Ok(()),
                    Err(returned_client) => {
                        client = returned_client;
                        continue;
                    }
                }
            }
            Ok(Err(e)) => {
                log::warn!("[{label}] worker passthrough {worker} -> {dst_ip}:{port} failed: {e}");
            }
            Err(_) => {
                log::warn!("[{label}] worker passthrough {worker} -> {dst_ip}:{port} timeout");
            }
        }
    }
    log::warn!(
        "[{label}] worker passthrough exhausted ({} worker(s)) -> front fallback",
        workers.len()
    );
    Err(client)
}

pub async fn blind_relay(
    client: TcpStream,
    orig_ip: &str,
    orig_port: u16,
    initial: &[u8],
    label: &str,
) {
    let http_payload = if orig_port == 80 {
        http_front_passthrough_payload(initial, orig_ip, orig_port)
    } else {
        initial.to_vec()
    };

    // Worker passthrough: TLS and media CDN HTTP (needs kws{N}-1 Host rewrite).
    // Regular DC MTProto-over-HTTP must use local FRONT_IP below — the Worker
    // path to real DC :80 returns HTTP 404 and the session ends before fallback.
    let client = if should_try_worker_passthrough(orig_port) {
        match try_worker_passthrough(client, orig_ip, orig_port, &http_payload, label).await {
            Ok(()) => return,
            Err(c) => c,
        }
    } else {
        log::info!("[{label}] HTTP :80 -> front passthrough (skip worker)");
        client
    };

    let ports = passthrough_ports(orig_port);
    let mut host = passthrough_host(initial);
    if host.is_empty() && orig_port == 80 {
        host = http_front_host(orig_ip, orig_port);
    }
    let front_payload = if orig_port == 80 {
        http_payload
    } else {
        initial.to_vec()
    };
    let wire_host = http_wire_host(&front_payload).or_else(|| {
        if host.is_empty() {
            None
        } else {
            Some(host.clone())
        }
    });
    let targets = passthrough_targets(orig_ip, &host, orig_port);

    let mut tried = Vec::new();
    let mut remote = None;
    let mut dst = String::new();

    'outer: for ip in &targets {
        for port in &ports {
            dst = format!("{ip}:{port}");
            tried.push(dst.clone());
            if let Ok(Ok(r)) = timeout(Duration::from_secs(5), TcpStream::connect(&dst)).await {
                tune_tcp(&r);
                remote = Some(r);
                break 'outer;
            }
        }
    }

    let Some(mut remote) = remote else {
        log::warn!("[{label}] passthrough failed host={host:?} (tried {tried:?})");
        return;
    };

    if let Some(h) = wire_host.as_deref() {
        if h != orig_ip {
            log::info!(
                "[{label}] passthrough -> {dst} host={h} ({} bytes initial)",
                front_payload.len()
            );
        } else if front_payload.len() != initial.len() {
            log::info!(
                "[{label}] passthrough -> {dst} media-http ({} bytes initial)",
                front_payload.len()
            );
        } else {
            log::info!(
                "[{label}] passthrough -> {dst} ({} bytes initial)",
                front_payload.len()
            );
        }
    } else {
        log::info!(
            "[{label}] passthrough -> {dst} ({} bytes initial)",
            front_payload.len()
        );
    }

    if !front_payload.is_empty() && remote.write_all(&front_payload).await.is_err() {
        return;
    }

    let (mut cr, mut cw) = client.into_split();
    let (mut rr, mut rw) = remote.into_split();
    let up = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut cr, &mut rw).await;
        let _ = rw.shutdown().await;
    });
    let mut peek = vec![0u8; 512];
    match rr.read(&mut peek).await {
        Ok(0) => {}
        Ok(n) => {
            log_http_response(label, &dst, &peek[..n]);
            if cw.write_all(&peek[..n]).await.is_err() {
                up.abort();
                return;
            }
            let _ = tokio::io::copy(&mut rr, &mut cw).await;
        }
        Err(e) => log::warn!("[{label}] passthrough read from {dst}: {e}"),
    }
    let _ = up.await;
}

/// HTTP Host as sent on the wire (includes `dc-ip:80`; unlike `parse_http_host`).
fn http_wire_host(data: &[u8]) -> Option<String> {
    let lower = String::from_utf8_lossy(data).to_lowercase();
    let i = lower.find("\r\nhost:")?;
    let rest = &data[i + 7..];
    let mut j = 0usize;
    while j < rest.len() && (rest[j] == b' ' || rest[j] == b'\t') {
        j += 1;
    }
    let start = j;
    while j < rest.len() && rest[j] != b'\r' && rest[j] != b'\n' {
        j += 1;
    }
    if j == start {
        return None;
    }
    Some(String::from_utf8_lossy(&rest[start..j]).trim().to_string())
}

fn log_http_response(label: &str, dst: &str, chunk: &[u8]) {
    let Ok(head) = std::str::from_utf8(chunk) else {
        log::info!(
            "[{label}] passthrough <- {dst} ({} bytes, non-utf8)",
            chunk.len()
        );
        return;
    };
    let status = head.lines().next().unwrap_or("<empty>");
    let location = head
        .lines()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .strip_prefix("location:")
                .map(|v| v.trim().to_string())
        })
        .unwrap_or_default();
    if location.is_empty() {
        log::info!("[{label}] passthrough <- {dst} {status}");
    } else {
        log::info!("[{label}] passthrough <- {dst} {status} Location={location}");
    }
}

fn fmt_dc(dc: i32, is_media: bool) -> String {
    if is_media {
        format!("DC{dc}m")
    } else {
        format!("DC{dc}")
    }
}

fn passthrough_ports(orig_port: u16) -> Vec<u16> {
    match orig_port {
        0 => vec![443],
        5222 => vec![5222, 443],
        p => vec![p],
    }
}

pub fn should_skip_ws(dc: i32, is_media: bool, orig_ip: &str) -> bool {
    if ws_blacklisted(dc, is_media) {
        return true;
    }
    let target = ws_target_ip(dc, orig_ip);
    should_skip_direct_ws(&target, dc)
}
