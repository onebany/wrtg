use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use aes::Aes256;
use ctr::cipher::{KeyIvInit, StreamCipher};
use rand::Rng;

type Aes256Ctr = ctr::Ctr128BE<Aes256>;

pub const HANDSHAKE_LEN: usize = 64;
const SKIP_LEN: usize = 8;
const PREKEY_LEN: usize = 32;
const IV_LEN: usize = 16;
const PROTO_TAG_POS: usize = 56;

pub const PROTO_ABRIDGED_INT: u32 = 0xEFEF_EFEF;
pub const PROTO_INTERMEDIATE_INT: u32 = 0xEEEE_EEEE;
pub const PROTO_PADDED_INTERMEDIATE_INT: u32 = 0xDDDD_DDDD;

pub const PROTO_TAG_ABRIDGED: [u8; 4] = [0xef, 0xef, 0xef, 0xef];
pub const PROTO_TAG_INTERMEDIATE: [u8; 4] = [0xee, 0xee, 0xee, 0xee];
pub const PROTO_TAG_SECURE: [u8; 4] = [0xdd, 0xdd, 0xdd, 0xdd];

/// MTProto transport max payload (16 MiB) plus largest length header.
pub const MAX_MTPROTO_PACKET: usize = 16 * 1024 * 1024 + 4;

/// WebSocket binary frame cap (slightly above MTProto max).
pub const MAX_WS_PAYLOAD: usize = 16 * 1024 * 1024 + 64;

static RESERVED_FIRST_BYTES: OnceLock<HashMap<u8, bool>> = OnceLock::new();
static RESERVED_STARTS: OnceLock<[[u8; 4]; 7]> = OnceLock::new();

fn reserved_first_bytes() -> &'static HashMap<u8, bool> {
    RESERVED_FIRST_BYTES.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert(0xEF, true);
        m
    })
}

fn reserved_starts() -> &'static [[u8; 4]; 7] {
    RESERVED_STARTS.get_or_init(|| {
        [
            [0x48, 0x45, 0x41, 0x44], // HEAD
            [0x50, 0x4F, 0x53, 0x54], // POST
            [0x47, 0x45, 0x54, 0x20], // GET
            [0xee, 0xee, 0xee, 0xee],
            [0xdd, 0xdd, 0xdd, 0xdd],
            [0x16, 0x03, 0x01, 0x02], // TLS
            [0, 0, 0, 0],
        ]
    })
}

static FRONT_IP: OnceLock<RwLock<String>> = OnceLock::new();
static DC_FRONT_IPS: OnceLock<RwLock<HashMap<i32, String>>> = OnceLock::new();
static FRONT_DCS: OnceLock<RwLock<Vec<i32>>> = OnceLock::new();

/// Default set of DCs the *global* FRONT_IP is applied to. The stock front
/// `149.154.167.220` only fronts DC2/DC4 web sockets (DC1/3/5 get HTTP 302),
/// so other DCs default to their real IP (direct / CF worker with correct dst).
/// Override via `WRTG_FRONT_DCS` (e.g. `all`, `none`, `1,2,3,4,5`).
const DEFAULT_FRONT_DCS: [i32; 2] = [2, 4];

fn dc_front_ips_cell() -> &'static RwLock<HashMap<i32, String>> {
    DC_FRONT_IPS.get_or_init(|| RwLock::new(HashMap::new()))
}

fn front_dcs_cell() -> &'static RwLock<Vec<i32>> {
    FRONT_DCS.get_or_init(|| RwLock::new(DEFAULT_FRONT_DCS.to_vec()))
}

pub fn set_front_dcs(dcs: Vec<i32>) {
    *front_dcs_cell().write().unwrap() = dcs;
}

/// Whether the global FRONT_IP is applied to `dc` (DC203 folds to DC2).
pub fn front_applies_to_dc(dc: i32) -> bool {
    let dc = if dc == 203 { 2 } else { dc };
    front_dcs_cell().read().unwrap().contains(&dc)
}

fn front_ip_cell() -> &'static RwLock<String> {
    FRONT_IP.get_or_init(|| RwLock::new("149.154.167.220".to_string()))
}

pub fn set_front_ip(ip: String) {
    *front_ip_cell().write().unwrap() = ip;
}

pub fn front_ip() -> String {
    front_ip_cell().read().unwrap().clone()
}

pub fn set_dc_front_ips(map: HashMap<i32, String>) {
    *dc_front_ips_cell().write().unwrap() = map;
}

pub fn set_dc_front_ip(dc: i32, ip: String) {
    dc_front_ips_cell().write().unwrap().insert(dc, ip);
}

pub fn dc_front_ip(dc: i32) -> String {
    let dc_key = if dc == 203 { 2 } else { dc };
    // Explicit per-DC override (DC{N}_FRONT_IP / WRTG_DC_IPS) always wins.
    if let Some(ip) = dc_front_ips_cell().read().unwrap().get(&dc_key) {
        if !ip.is_empty() {
            return ip.clone();
        }
    }
    if let Some(ip) = dc_front_ips_cell().read().unwrap().get(&dc) {
        if !ip.is_empty() {
            return ip.clone();
        }
    }
    // Global FRONT_IP only for in-scope DCs; others resolve to their real IP
    // (see `ws_target_ip`), which keeps direct + CF worker routing correct.
    if front_applies_to_dc(dc) {
        return front_ip();
    }
    String::new()
}

pub fn dc_default_ip(dc: i32) -> Option<&'static str> {
    let dc = if dc == 203 { 2 } else { dc };
    dc_default_ips().get(&dc).copied()
}

fn dc_default_ips() -> &'static HashMap<i32, &'static str> {
    static MAP: OnceLock<HashMap<i32, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        HashMap::from([
            (1, "149.154.175.50"),
            (2, "149.154.167.51"),
            (3, "149.154.175.100"),
            (4, "149.154.167.91"),
            (5, "149.154.171.5"),
            (203, "91.105.192.100"),
        ])
    })
}

#[derive(Clone, Copy)]
pub struct DcAltEntry {
    pub dc: i32,
    pub is_media: bool,
}

pub fn dc_alt_ips() -> &'static HashMap<&'static str, DcAltEntry> {
    static MAP: OnceLock<HashMap<&'static str, DcAltEntry>> = OnceLock::new();
    MAP.get_or_init(|| {
        HashMap::from([
            (
                "149.154.162.123",
                DcAltEntry {
                    dc: 2,
                    is_media: true,
                },
            ),
            (
                "149.154.175.211",
                DcAltEntry {
                    dc: 1,
                    is_media: true,
                },
            ),
            (
                "149.154.171.255",
                DcAltEntry {
                    dc: 5,
                    is_media: true,
                },
            ),
            // DC5 animated-emoji / sticker CDN (Telegram Desktop) — HTTP :80 needs
            // Host rewrite to kws5-1.web.telegram.org, else emoji render as blue placeholders.
            (
                "91.108.56.155",
                DcAltEntry {
                    dc: 5,
                    is_media: true,
                },
            ),
            (
                "149.154.175.58",
                DcAltEntry {
                    dc: 1,
                    is_media: false,
                },
            ),
            (
                "149.154.175.53",
                DcAltEntry {
                    dc: 1,
                    is_media: false,
                },
            ),
            (
                "149.154.167.41",
                DcAltEntry {
                    dc: 2,
                    is_media: false,
                },
            ),
            (
                "149.154.167.50",
                DcAltEntry {
                    dc: 2,
                    is_media: false,
                },
            ),
            // DC2 main endpoint seen on Telegram for Android (Pixel) — not embedded
            // in the obfuscated handshake, so needs orig-dst → DC mapping here or it
            // falls to blind passthrough via the slow CF worker instead of the fast front.
            (
                "149.154.167.35",
                DcAltEntry {
                    dc: 2,
                    is_media: false,
                },
            ),
        ])
    })
}

pub fn valid_dc(dc: i32) -> bool {
    (1..=5).contains(&dc) || dc == 203
}

pub fn ws_target_ip(dc: i32, orig_ip: &str) -> String {
    let front = dc_front_ip(dc);
    if !front.is_empty() {
        return front;
    }
    if !orig_ip.is_empty() {
        return orig_ip.to_string();
    }
    dc_default_ips()
        .get(&dc)
        .map(|s| (*s).to_string())
        .unwrap_or_default()
}

/// Whether an all-HTTP-302 WS outcome should trigger the per-DC blacklist.
///
/// On the stock `149.154.167.220` front, DC1/3/5 always get HTTP 302 — that is
/// expected and means "use CF worker", not "blacklist direct WS for 45 minutes".
pub fn ws_redirect_blacklist_warranted(dc: i32, target_ip: &str) -> bool {
    if target_ip.is_empty() {
        return true;
    }
    let dc_key = if dc == 203 { 2 } else { dc };
    let front = dc_front_ip(dc_key);
    if front.is_empty() || front != target_ip {
        return true;
    }
    target_ip == front_ip() && matches!(dc_key, 2 | 4)
}

pub fn tcp_fallback_targets(
    dc: i32,
    orig_ip: &str,
    is_media: bool,
    blocked_cdn: bool,
    ws_blacklisted: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut add = |ip: &str| {
        if ip.is_empty() || out.iter().any(|x| x == ip) {
            return;
        }
        out.push(ip.to_string());
    };

    // After WS blacklist on media CDN, prefer front IP TCP (not blind relay to CDN).
    if is_media && ws_blacklisted {
        add(&ws_target_ip(dc, orig_ip));
        if !orig_ip.is_empty() && dc_alt_ips().contains_key(orig_ip) {
            add(orig_ip);
        }
        return out;
    }

    if is_media && !orig_ip.is_empty() && !blocked_cdn && dc_alt_ips().contains_key(orig_ip) {
        add(orig_ip);
    }
    add(&ws_target_ip(dc, orig_ip));
    out
}

pub fn ws_domains(dc: i32, is_media: bool) -> Vec<String> {
    let dc = if dc == 203 { 2 } else { dc };
    // Media prefers the kws{N}-1 CDN host first; non-media prefers the base host.
    // (Matches the reference tg-ws-proxy ordering.)
    if is_media {
        vec![
            format!("kws{dc}-1.web.telegram.org"),
            format!("kws{dc}.web.telegram.org"),
        ]
    } else {
        vec![
            format!("kws{dc}.web.telegram.org"),
            format!("kws{dc}-1.web.telegram.org"),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct HandshakeInfo {
    pub dc: i32,
    pub is_media: bool,
    pub dc_in_pkt: bool,
    pub proto_int: u32,
    pub handshake: [u8; HANDSHAKE_LEN],
}

/// Curated (compiled-in) IP → DC lookup. Authoritative; checked before any
/// runtime-learned mapping.
pub(crate) fn dc_from_hardcoded(ip: &str) -> Option<(i32, bool)> {
    if let Some(ep) = dc_alt_ips().get(ip) {
        return Some((ep.dc, ep.is_media));
    }
    for (id, addr) in dc_default_ips() {
        if *addr == ip {
            return Some((*id, false));
        }
    }
    None
}

/// Resolve a Telegram datacenter from the original destination IP. Falls back to
/// the self-learning map ([`crate::dc_learn`]) for IPs Telegram has added since
/// this binary was built.
pub fn dc_from_orig_dst(ip: &str) -> Option<(i32, bool)> {
    dc_from_hardcoded(ip).or_else(|| crate::dc_learn::lookup(ip))
}

fn new_ctr(key: &[u8], iv: &[u8]) -> Result<Aes256Ctr, aes::cipher::InvalidLength> {
    Aes256Ctr::new_from_slices(key, iv)
}

/// Decrypts a direct (non-MTProxy) obfuscated2 init packet.
pub fn parse_direct_handshake(handshake: &[u8]) -> Result<HandshakeInfo, String> {
    if handshake.len() != HANDSHAKE_LEN {
        return Err(format!("handshake length {}", handshake.len()));
    }

    let dec_key = &handshake[SKIP_LEN..SKIP_LEN + PREKEY_LEN];
    let dec_iv = &handshake[SKIP_LEN + PREKEY_LEN..SKIP_LEN + PREKEY_LEN + IV_LEN];

    let mut stream = new_ctr(dec_key, dec_iv).map_err(|e| e.to_string())?;

    let mut skip = vec![0u8; PROTO_TAG_POS];
    stream.apply_keystream(&mut skip);

    let mut tail = handshake[PROTO_TAG_POS..].to_vec();
    stream.apply_keystream(&mut tail);

    let tag: [u8; 4] = tail[..4].try_into().unwrap();
    if tag != PROTO_TAG_ABRIDGED && tag != PROTO_TAG_INTERMEDIATE && tag != PROTO_TAG_SECURE {
        return Err(format!("unknown proto tag {tag:02x?}"));
    }

    let dc_idx = i16::from_le_bytes(tail[4..6].try_into().unwrap());
    let mut dc = dc_idx as i32;
    let mut is_media = false;
    let mut dc_in_pkt = false;
    if dc < 0 {
        is_media = true;
        dc = -dc;
    }
    if valid_dc(dc) {
        dc_in_pkt = true;
    } else {
        dc = 0;
        is_media = false;
    }

    let proto_int = match tag {
        PROTO_TAG_ABRIDGED => PROTO_ABRIDGED_INT,
        PROTO_TAG_INTERMEDIATE => PROTO_INTERMEDIATE_INT,
        _ => PROTO_PADDED_INTERMEDIATE_INT,
    };

    let mut hs = [0u8; HANDSHAKE_LEN];
    hs.copy_from_slice(handshake);

    Ok(HandshakeInfo {
        dc,
        is_media,
        dc_in_pkt,
        proto_int,
        handshake: hs,
    })
}

pub fn generate_relay_init(proto_tag: [u8; 4], dc_idx: i16) -> Result<Vec<u8>, String> {
    let mut rng = rand::rng();
    loop {
        let mut rnd = vec![0u8; HANDSHAKE_LEN];
        rng.fill_bytes(&mut rnd);

        if reserved_first_bytes().contains_key(&rnd[0]) {
            continue;
        }
        let start = [rnd[0], rnd[1], rnd[2], rnd[3]];
        if reserved_starts().contains(&start) {
            continue;
        }
        if [rnd[4], rnd[5], rnd[6], rnd[7]] == [0, 0, 0, 0] {
            continue;
        }

        let enc_key = rnd[SKIP_LEN..SKIP_LEN + PREKEY_LEN].to_vec();
        let enc_iv = rnd[SKIP_LEN + PREKEY_LEN..SKIP_LEN + PREKEY_LEN + IV_LEN].to_vec();

        let mut stream = new_ctr(&enc_key, &enc_iv).map_err(|e| e.to_string())?;
        let mut encrypted_full = rnd.clone();
        stream.apply_keystream(&mut encrypted_full);

        let dc_bytes = (dc_idx as u16).to_le_bytes();
        let mut tail_plain = Vec::with_capacity(8);
        tail_plain.extend_from_slice(&proto_tag);
        tail_plain.extend_from_slice(&dc_bytes);
        tail_plain.push(rnd[62]);
        tail_plain.push(rnd[63]);

        let mut encrypted_tail = [0u8; 8];
        for i in 0..8 {
            encrypted_tail[i] =
                tail_plain[i] ^ encrypted_full[PROTO_TAG_POS + i] ^ rnd[PROTO_TAG_POS + i];
        }

        rnd[PROTO_TAG_POS..PROTO_TAG_POS + 8].copy_from_slice(&encrypted_tail);
        return Ok(rnd);
    }
}

pub struct CryptoCtx {
    clt_dec: Aes256Ctr,
    clt_enc: Aes256Ctr,
    tg_enc: Aes256Ctr,
    tg_dec: Aes256Ctr,
}

/// Client -> Telegram cipher halves (one tokio task).
pub struct CryptoUp {
    clt_dec: Aes256Ctr,
    tg_enc: Aes256Ctr,
}

/// Telegram -> client cipher halves (other tokio task).
pub struct CryptoDown {
    tg_dec: Aes256Ctr,
    clt_enc: Aes256Ctr,
}

/// Re-encrypt `input` by running it through two AES-CTR keystreams in order
/// (decrypt with `a`, re-encrypt with `b`). Shared by every relay direction.
fn recrypt(a: &mut Aes256Ctr, b: &mut Aes256Ctr, input: &[u8]) -> Vec<u8> {
    let mut out = input.to_vec();
    a.apply_keystream(&mut out);
    b.apply_keystream(&mut out);
    out
}

impl CryptoCtx {
    pub fn split(self) -> (CryptoUp, CryptoDown) {
        (
            CryptoUp {
                clt_dec: self.clt_dec,
                tg_enc: self.tg_enc,
            },
            CryptoDown {
                tg_dec: self.tg_dec,
                clt_enc: self.clt_enc,
            },
        )
    }

    pub fn build_direct(handshake: &[u8], relay_init: &[u8]) -> Result<Self, String> {
        let clt_dec_key = &handshake[SKIP_LEN..SKIP_LEN + PREKEY_LEN];
        let clt_dec_iv = &handshake[SKIP_LEN + PREKEY_LEN..SKIP_LEN + PREKEY_LEN + IV_LEN];

        let prekey_iv = &handshake[SKIP_LEN..SKIP_LEN + PREKEY_LEN + IV_LEN];
        let mut rev = prekey_iv.to_vec();
        rev.reverse();
        let clt_enc_key = &rev[..PREKEY_LEN];
        let clt_enc_iv = &rev[PREKEY_LEN..];

        let mut clt_dec = new_ctr(clt_dec_key, clt_dec_iv).map_err(|e| e.to_string())?;
        let clt_enc = new_ctr(clt_enc_key, clt_enc_iv).map_err(|e| e.to_string())?;

        let mut zero64 = vec![0u8; HANDSHAKE_LEN];
        clt_dec.apply_keystream(&mut zero64);

        let relay_enc_key = &relay_init[SKIP_LEN..SKIP_LEN + PREKEY_LEN];
        let relay_enc_iv = &relay_init[SKIP_LEN + PREKEY_LEN..SKIP_LEN + PREKEY_LEN + IV_LEN];

        let relay_prekey_iv = &relay_init[SKIP_LEN..SKIP_LEN + PREKEY_LEN + IV_LEN];
        let mut relay_rev = relay_prekey_iv.to_vec();
        relay_rev.reverse();
        let relay_dec_key = &relay_rev[..PREKEY_LEN];
        let relay_dec_iv = &relay_rev[PREKEY_LEN..];

        let mut tg_enc = new_ctr(relay_enc_key, relay_enc_iv).map_err(|e| e.to_string())?;
        let tg_dec = new_ctr(relay_dec_key, relay_dec_iv).map_err(|e| e.to_string())?;

        let mut zero64b = vec![0u8; HANDSHAKE_LEN];
        tg_enc.apply_keystream(&mut zero64b);

        Ok(Self {
            clt_dec,
            clt_enc,
            tg_enc,
            tg_dec,
        })
    }

    pub fn client_to_telegram(&mut self, input: &[u8]) -> Vec<u8> {
        recrypt(&mut self.clt_dec, &mut self.tg_enc, input)
    }

    pub fn telegram_to_client(&mut self, input: &[u8]) -> Vec<u8> {
        recrypt(&mut self.tg_dec, &mut self.clt_enc, input)
    }
}

impl CryptoUp {
    pub fn client_to_telegram(&mut self, input: &[u8]) -> Vec<u8> {
        recrypt(&mut self.clt_dec, &mut self.tg_enc, input)
    }
}

impl CryptoDown {
    pub fn telegram_to_client(&mut self, input: &[u8]) -> Vec<u8> {
        recrypt(&mut self.tg_dec, &mut self.clt_enc, input)
    }
}

/// CTR stream for msg_splitter (relay-side decrypt).
pub fn new_relay_decrypt_stream(relay_init: &[u8]) -> Result<Aes256Ctr, String> {
    let enc_key = &relay_init[SKIP_LEN..SKIP_LEN + PREKEY_LEN];
    let enc_iv = &relay_init[SKIP_LEN + PREKEY_LEN..SKIP_LEN + PREKEY_LEN + IV_LEN];
    let mut dec = new_ctr(enc_key, enc_iv).map_err(|e| e.to_string())?;
    let mut zero64 = vec![0u8; HANDSHAKE_LEN];
    dec.apply_keystream(&mut zero64);
    Ok(dec)
}

pub fn proto_tag_for(proto_int: u32) -> [u8; 4] {
    match proto_int {
        PROTO_ABRIDGED_INT => PROTO_TAG_ABRIDGED,
        PROTO_INTERMEDIATE_INT => PROTO_TAG_INTERMEDIATE,
        _ => PROTO_TAG_SECURE,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static FRONT_IP_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn with_front_ip<F: FnOnce()>(front: &str, f: F) {
        let _guard = FRONT_IP_TEST_LOCK.lock().unwrap();
        let prev = front_ip();
        let prev_dcs = front_dcs_cell().read().unwrap().clone();
        set_front_ip(front.to_string());
        set_front_dcs(DEFAULT_FRONT_DCS.to_vec());
        f();
        set_front_dcs(prev_dcs);
        set_front_ip(prev);
    }

    #[test]
    fn relay_init_roundtrip() {
        let relay = generate_relay_init(PROTO_TAG_ABRIDGED, 2).unwrap();
        assert_eq!(relay.len(), HANDSHAKE_LEN);
        let info = parse_direct_handshake(&relay).unwrap();
        assert_eq!(info.dc, 2);
        assert!(!info.is_media);
        assert!(info.dc_in_pkt);
        assert_eq!(info.proto_int, PROTO_ABRIDGED_INT);
    }

    #[test]
    fn relay_init_media_dc() {
        let relay = generate_relay_init(PROTO_TAG_INTERMEDIATE, -2).unwrap();
        let info = parse_direct_handshake(&relay).unwrap();
        assert_eq!(info.dc, 2);
        assert!(info.is_media);
    }

    #[test]
    fn crypto_ctx_builds() {
        let client_hs = generate_relay_init(PROTO_TAG_ABRIDGED, 3).unwrap();
        let relay_init = generate_relay_init(PROTO_TAG_ABRIDGED, 3).unwrap();
        let mut ctx = CryptoCtx::build_direct(&client_hs, &relay_init).unwrap();
        let out = ctx.client_to_telegram(b"probe");
        assert_eq!(out.len(), 5);
        assert_ne!(out, b"probe");
    }

    #[test]
    fn dc_from_orig_dst_known() {
        let (dc, media) = dc_from_orig_dst("149.154.162.123").unwrap();
        assert_eq!(dc, 2);
        assert!(media);
        let (dc, media) = dc_from_orig_dst("149.154.175.50").unwrap();
        assert_eq!(dc, 1);
        assert!(!media);
        // DC2 main endpoint added for Telegram/Android (Pixel) orig-dst mapping.
        let (dc, media) = dc_from_orig_dst("149.154.167.35").unwrap();
        assert_eq!(dc, 2);
        assert!(!media);
    }

    #[test]
    fn tcp_fallback_uses_front_ip() {
        // DC4 is in the default front scope {2,4}.
        with_front_ip("149.154.167.220", || {
            let targets = tcp_fallback_targets(4, "149.154.167.91", false, false, false);
            assert_eq!(targets, vec!["149.154.167.220".to_string()]);
        });
    }

    #[test]
    fn tcp_fallback_media_cdn_prefers_orig() {
        // DC2 media (in front scope): prefer orig CDN IP, then front.
        with_front_ip("149.154.167.220", || {
            let targets = tcp_fallback_targets(2, "149.154.162.123", true, false, false);
            assert_eq!(
                targets,
                vec!["149.154.162.123".to_string(), "149.154.167.220".to_string()]
            );
        });
    }

    #[test]
    fn tcp_fallback_media_blacklisted_prefers_front() {
        with_front_ip("149.154.167.220", || {
            let targets = tcp_fallback_targets(2, "149.154.162.123", true, true, true);
            assert_eq!(targets[0], "149.154.167.220");
            assert!(targets.contains(&"149.154.162.123".to_string()));
        });
    }

    #[test]
    fn dc_front_ip_override() {
        let _guard = FRONT_IP_TEST_LOCK.lock().unwrap();
        let prev = front_ip();
        let prev_dc = dc_front_ips_cell().read().unwrap().clone();
        let prev_dcs = front_dcs_cell().read().unwrap().clone();
        set_front_ip("149.154.167.220".to_string());
        set_front_dcs(DEFAULT_FRONT_DCS.to_vec());
        // Explicit per-DC override wins even for an out-of-scope DC (DC1).
        set_dc_front_ip(1, "1.2.3.4".to_string());
        assert_eq!(dc_front_ip(1), "1.2.3.4");
        assert_eq!(dc_front_ip(2), "149.154.167.220"); // in scope -> global front
        assert_eq!(ws_target_ip(1, ""), "1.2.3.4");
        set_dc_front_ips(prev_dc);
        set_front_dcs(prev_dcs);
        set_front_ip(prev);
    }

    #[test]
    fn front_scope_default_only_dc2_dc4() {
        with_front_ip("149.154.167.220", || {
            // DC2/DC4 fronted; DC1/DC3/DC5 resolve to their real IP (direct).
            assert!(front_applies_to_dc(2));
            assert!(front_applies_to_dc(4));
            assert!(front_applies_to_dc(203)); // folds to DC2
            assert!(!front_applies_to_dc(1));
            assert!(!front_applies_to_dc(3));
            assert!(!front_applies_to_dc(5));
            assert_eq!(dc_front_ip(1), "");
            assert_eq!(ws_target_ip(1, "149.154.175.53"), "149.154.175.53");
            assert_eq!(ws_target_ip(4, "149.154.167.91"), "149.154.167.220");
        });
    }

    #[test]
    fn front_scope_all_and_none() {
        let _guard = FRONT_IP_TEST_LOCK.lock().unwrap();
        let prev = front_ip();
        let prev_dcs = front_dcs_cell().read().unwrap().clone();
        set_front_ip("149.154.167.220".to_string());

        set_front_dcs(vec![1, 2, 3, 4, 5]);
        assert_eq!(ws_target_ip(1, "149.154.175.53"), "149.154.167.220");
        assert_eq!(ws_target_ip(5, "149.154.171.5"), "149.154.167.220");

        set_front_dcs(Vec::new());
        assert_eq!(ws_target_ip(2, "149.154.167.51"), "149.154.167.51");

        set_front_dcs(prev_dcs);
        set_front_ip(prev);
    }

    #[test]
    fn ws_domains_media_ordering() {
        assert_eq!(
            ws_domains(4, false),
            vec![
                "kws4.web.telegram.org".to_string(),
                "kws4-1.web.telegram.org".to_string()
            ]
        );
        assert_eq!(
            ws_domains(4, true),
            vec![
                "kws4-1.web.telegram.org".to_string(),
                "kws4.web.telegram.org".to_string()
            ]
        );
        // dc203 folds to kws2
        assert_eq!(ws_domains(203, false)[0], "kws2.web.telegram.org");
    }

    #[test]
    fn dc_default_ip_maps_dc203_to_dc2() {
        assert_eq!(dc_default_ip(203), Some("149.154.167.51"));
        assert_eq!(dc_default_ip(5), Some("149.154.171.5"));
    }

    #[test]
    fn ws_target_ip_uses_front_ip() {
        // DC2/DC4 are in the default front scope; DC1 (out of scope) stays direct.
        with_front_ip("149.154.167.220", || {
            assert_eq!(ws_target_ip(2, "149.154.167.51"), "149.154.167.220");
            assert_eq!(ws_target_ip(4, ""), "149.154.167.220");
            assert_eq!(ws_target_ip(1, "149.154.175.50"), "149.154.175.50");
        });
    }

    #[test]
    fn ws_target_ip_fallback() {
        with_front_ip("", || {
            assert_eq!(ws_target_ip(2, "149.154.167.51"), "149.154.167.51");
            assert_eq!(ws_target_ip(3, ""), "149.154.175.100");
        });
    }

    #[test]
    fn ws_redirect_blacklist_warranted_stock_front() {
        with_front_ip("149.154.167.220", || {
            assert!(ws_redirect_blacklist_warranted(2, "149.154.167.220"));
            assert!(ws_redirect_blacklist_warranted(4, "149.154.167.220"));
            let prev_dc_front = dc_front_ips_cell().read().unwrap().clone();
            set_dc_front_ip(3, "149.154.167.220".to_string());
            set_dc_front_ip(5, "149.154.167.220".to_string());
            assert!(!ws_redirect_blacklist_warranted(3, "149.154.167.220"));
            assert!(!ws_redirect_blacklist_warranted(5, "149.154.167.220"));
            assert!(ws_redirect_blacklist_warranted(3, "149.154.175.100"));
            *dc_front_ips_cell().write().unwrap() = prev_dc_front;

            let prev_dcs = front_dcs_cell().read().unwrap().clone();
            set_front_dcs(vec![1, 2, 3, 4, 5]);
            assert!(!ws_redirect_blacklist_warranted(1, "149.154.167.220"));
            set_front_dcs(prev_dcs);
        });
    }

    #[test]
    fn crypto_matches_go_deterministic() {
        let client = hex::decode(
            "5514212e050607086f7c8996a3b0bdcad7e4f1fe0b1825323f4c596673808d9aa7b4c1cedbe8f5020f1c293643505d6a7784919eabb8c5d214afaf44beeb4655",
        )
        .unwrap();
        let relay = hex::decode(
            "4230415201020304a7b8c9daebfc0d1e2f405162738495a6b7c8d9eafb0c1d2e3f5061728394a5b6c7d8e9fa0b1c2d3e4f60718293a4b5c6a3a408a01644ebfc",
        )
        .unwrap();
        let mut ctx = CryptoCtx::build_direct(&client, &relay).unwrap();
        let inb = [0x10u8, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80];
        let up = ctx.client_to_telegram(&inb);
        let down = ctx.telegram_to_client(&inb);
        assert_eq!(hex::encode(up), "44d1d015a8014224");
        assert_eq!(hex::encode(down), "e399df8275a89202");
    }
}
