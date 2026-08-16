//! PPP inside the FortiGate `0x5050` envelope (FG-PPP-01).
//!
//! **Why this exists.** The first cut of this module assumed the FortiGate "v2"
//! wire protocol, where the framed payload is a bare IP packet. A packet capture
//! against a live FortiOS 7.x gateway (`sslvpn-tunnel ver='2'`, advertising both
//! `<tunnel-method value='ppp'/>` and `<tunnel-method value='tun'/>`) showed that
//! `GET /remote/sslvpn-tunnel` still yields **v1**: the server sends no HTTP
//! response at all, waits for the client to speak first, and then negotiates
//! plain PPP. Neither openfortivpn nor OpenConnect implements v2, and no public
//! source documents how a client would ask for it.
//!
//! **Wire format**, confirmed byte-for-byte on the wire:
//!
//! ```text
//! [BE u16 total = 6 + body_len][BE u16 0x5050][BE u16 body_len][body]
//! body = [BE u16 PPP protocol][protocol payload]
//! ```
//!
//! There is **no HDLC framing and no `FF 03` address/control prefix** — the
//! server omits it, though it tolerates receiving one. This matches OpenConnect's
//! `PPP_ENCAP_FORTINET` (`encap_len = 6`, `hdlc = 0`).
//!
//! Observed negotiation (gateway `ver='2'`, FortiOS 7.x):
//!
//! ```text
//! -> LCP  Conf-Req id=1 [MRU=1354, Magic=<ours>]
//! <- LCP  Conf-Req id=1 [Magic=<theirs>]
//! -> LCP  Conf-Ack id=1 [Magic=<theirs>]
//! <- LCP  Conf-Ack id=1 [MRU=1354, Magic=<ours>]
//! -> IPCP Conf-Req id=1 [IP=0.0.0.0, DNS1=0.0.0.0, DNS2=0.0.0.0]
//! <- IPCP Conf-Req id=1 [IP=<peer>]
//! -> IPCP Conf-Ack id=1 [IP=<peer>]
//! <- IPCP Conf-Nak id=1 [IP=<ours>, DNS1=..., DNS2=...]
//! -> IPCP Conf-Req id=2 [IP=<ours>, DNS1=..., DNS2=...]
//! <- IPCP Conf-Ack id=2 [...]
//! ```
//!
//! After that, protocol `0x0021` frames carry raw IPv4 both ways.
//!
//! Every byte parsed here is untrusted server input: parsing is bounded, total,
//! and never panics or indexes past the slice.
#![allow(dead_code)]

use std::net::Ipv4Addr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::framing;
use crate::error::VpnError;

// ---------------------------------------------------------------------------
// Protocol / code constants
// ---------------------------------------------------------------------------

pub const PPP_LCP: u16 = 0xc021;
pub const PPP_IPCP: u16 = 0x8021;
pub const PPP_IPV4: u16 = 0x0021;
pub const PPP_PAP: u16 = 0xc023;
pub const PPP_CHAP: u16 = 0xc223;

pub const CODE_CONF_REQ: u8 = 1;
pub const CODE_CONF_ACK: u8 = 2;
pub const CODE_CONF_NAK: u8 = 3;
pub const CODE_CONF_REJ: u8 = 4;
pub const CODE_TERM_REQ: u8 = 5;
pub const CODE_TERM_ACK: u8 = 6;
pub const CODE_CODE_REJ: u8 = 7;
pub const CODE_PROTO_REJ: u8 = 8;
pub const CODE_ECHO_REQ: u8 = 9;
pub const CODE_ECHO_REP: u8 = 10;

/// LCP option: Maximum-Receive-Unit.
pub const LCP_OPT_MRU: u8 = 1;
/// LCP option: Authentication-Protocol. FortiGate authenticates over HTTPS
/// before the tunnel, so a server asking for PAP/CHAP here is unsupported.
pub const LCP_OPT_AUTH: u8 = 3;
/// LCP option: Magic-Number (loopback detection, echoed in Echo-Request/Reply).
pub const LCP_OPT_MAGIC: u8 = 5;

/// IPCP option: IP-Address.
pub const IPCP_OPT_ADDR: u8 = 3;
/// IPCP option: Primary DNS server (RFC 1877).
pub const IPCP_OPT_DNS1: u8 = 129;
/// IPCP option: Secondary DNS server (RFC 1877).
pub const IPCP_OPT_DNS2: u8 = 131;

/// MRU we ask for. Matches the `mru 1354` that openfortivpn passes to `pppd`:
/// 1500 Ethernet - 20 IP - 20 TCP - 6 FortiGate header - TLS record overhead.
pub const DEFAULT_MRU: u16 = 1354;

/// How long to wait for any single peer frame during negotiation.
const RECV_TIMEOUT: Duration = Duration::from_secs(5);
/// Retransmit our outstanding Configure-Request this often (RFC 1661 Restart timer).
const RESTART_INTERVAL: Duration = Duration::from_secs(3);
/// Give the whole LCP+IPCP handshake this long before failing.
const NEGOTIATE_DEADLINE: Duration = Duration::from_secs(30);
/// Bound on negotiation rounds, so a peer that Naks forever cannot spin us.
const MAX_ROUNDS: u32 = 40;

// ---------------------------------------------------------------------------
// Control-protocol packet + option codec (pure)
// ---------------------------------------------------------------------------

/// A parsed PPP control packet: `[code][id][BE u16 length][data...]`, where
/// `length` covers the 4-byte header plus `data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpPacket {
    pub code: u8,
    pub id: u8,
    /// Payload after the 4-byte header, already truncated to the declared length.
    pub data: Vec<u8>,
}

/// Parse a control packet. Rejects a truncated header or a `length` that
/// overruns the buffer, rather than trusting the server's field.
pub fn parse_cp(b: &[u8]) -> Result<CpPacket, VpnError> {
    if b.len() < 4 {
        return Err(VpnError::Protocol(format!(
            "PPP control packet too short ({} bytes)",
            b.len()
        )));
    }
    let len = u16::from_be_bytes([b[2], b[3]]) as usize;
    if len < 4 || len > b.len() {
        return Err(VpnError::Protocol(format!(
            "PPP control length {len} inconsistent with {} available bytes",
            b.len()
        )));
    }
    Ok(CpPacket { code: b[0], id: b[1], data: b[4..len].to_vec() })
}

/// Build a control packet from a code, id, and option/payload bytes.
pub fn build_cp(code: u8, id: u8, data: &[u8]) -> Vec<u8> {
    let len = (data.len() + 4) as u16;
    let mut out = Vec::with_capacity(data.len() + 4);
    out.push(code);
    out.push(id);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(data);
    out
}

/// Parse a `[type][len][value...]` option list. Stops cleanly at the first
/// malformed entry instead of erroring — a peer that appends junk still gets its
/// well-formed options honored, and nothing here can panic.
pub fn parse_options(b: &[u8]) -> Vec<(u8, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 2 <= b.len() {
        let typ = b[i];
        let len = b[i + 1] as usize;
        if len < 2 || i + len > b.len() {
            break;
        }
        out.push((typ, b[i + 2..i + len].to_vec()));
        i += len;
    }
    out
}

/// Serialize an option list. An option whose value exceeds 253 bytes cannot be
/// expressed (the length byte covers type+len+value) and is skipped.
pub fn build_options(opts: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (typ, val) in opts {
        if val.len() > 253 {
            continue;
        }
        out.push(*typ);
        out.push((val.len() + 2) as u8);
        out.extend_from_slice(val);
    }
    out
}

/// Read a 4-byte option value as an IPv4 address.
fn opt_ipv4(v: &[u8]) -> Option<Ipv4Addr> {
    (v.len() == 4).then(|| Ipv4Addr::new(v[0], v[1], v[2], v[3]))
}

/// A cheap magic number without pulling in `rand` (deps are LOCKED). Only needs
/// to be unlikely to collide with the peer's — it is loopback detection, not a
/// security value.
fn gen_magic() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ (d.as_secs() as u32))
        .unwrap_or(0x59454C4C);
    nanos | 1 // never zero: 0 means "no magic number" in RFC 1661
}

// ---------------------------------------------------------------------------
// Framing helpers (envelope + PPP protocol field)
// ---------------------------------------------------------------------------

/// Frame a PPP protocol payload for the wire.
pub fn encode_ppp(proto: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(framing::FORTI_HEADER_LEN + 2 + payload.len());
    encode_ppp_append(proto, payload, &mut out);
    out
}

/// Append a framed PPP payload to a caller-owned buffer (hot path: no per-packet
/// allocation, and several frames coalesce into one TLS write).
pub fn encode_ppp_append(proto: u16, payload: &[u8], out: &mut Vec<u8>) {
    let body_len = 2 + payload.len();
    let total = (framing::FORTI_HEADER_LEN + body_len) as u16;
    out.reserve(framing::FORTI_HEADER_LEN + body_len);
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&framing::FORTI_MAGIC.to_be_bytes());
    out.extend_from_slice(&(body_len as u16).to_be_bytes());
    out.extend_from_slice(&proto.to_be_bytes());
    out.extend_from_slice(payload);
}

/// Split an envelope body into `(protocol, payload)`, tolerating a leading
/// HDLC `FF 03` that some peers still emit.
pub fn split_ppp(body: &[u8]) -> Option<(u16, &[u8])> {
    let off = if body.len() >= 2 && body[0] == 0xFF && body[1] == 0x03 { 2 } else { 0 };
    if body.len() < off + 2 {
        return None;
    }
    Some((u16::from_be_bytes([body[off], body[off + 1]]), &body[off + 2..]))
}

/// Decode one complete frame from `buf`, returning `(protocol, payload)`.
/// `Ok(None)` means more bytes are needed; the buffer is left untouched, which
/// keeps the caller's read loop cancellation-safe.
pub fn try_decode_ppp(buf: &mut BytesMut) -> Result<Option<(u16, Vec<u8>)>, VpnError> {
    let Some(body) = framing::try_decode_frame(buf)? else {
        return Ok(None);
    };
    match split_ppp(&body) {
        Some((proto, payload)) => Ok(Some((proto, payload.to_vec()))),
        None => Err(VpnError::Protocol(format!(
            "PPP frame body too short ({} bytes)",
            body.len()
        ))),
    }
}

/// Build an LCP Echo-Request carrying our magic number (client-side liveness).
pub fn echo_request(magic: u32, id: u8) -> Vec<u8> {
    encode_ppp(PPP_LCP, &build_cp(CODE_ECHO_REQ, id, &magic.to_be_bytes()))
}

/// Build the LCP Echo-Reply answering `req` (the peer's Echo-Request payload).
/// RFC 1661: the reply carries OUR magic number, then the request's data.
pub fn echo_reply(magic: u32, req: &CpPacket) -> Vec<u8> {
    let mut data = magic.to_be_bytes().to_vec();
    if req.data.len() > 4 {
        data.extend_from_slice(&req.data[4..]);
    }
    encode_ppp(PPP_LCP, &build_cp(CODE_ECHO_REP, req.id, &data))
}

/// Build an LCP Terminate-Request (polite shutdown).
pub fn terminate_request(id: u8) -> Vec<u8> {
    encode_ppp(PPP_LCP, &build_cp(CODE_TERM_REQ, id, b"bye"))
}

// ---------------------------------------------------------------------------
// Negotiation
// ---------------------------------------------------------------------------

/// What LCP + IPCP agreed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PppSession {
    /// Our tunnel address, from the IPCP Configure-Nak/Ack.
    pub address: Ipv4Addr,
    /// The gateway's tunnel address, from its IPCP Configure-Request.
    pub peer: Option<Ipv4Addr>,
    /// DNS servers offered over IPCP (may be empty; the config XML also has them).
    pub dns: Vec<Ipv4Addr>,
    /// Agreed MRU — the tunnel MTU.
    pub mtu: u16,
    /// Our LCP magic number; the forwarding loop needs it for Echo frames.
    pub magic: u32,
}

/// Strip a leading `HTTP/1.1 ...\r\n\r\n` block from the tunnel stream.
///
/// The reference gateway answers `GET /remote/sslvpn-tunnel` with silence, but
/// others reply with a status line first and only then switch to framed data
/// (OpenConnect calls this `check_http_response`). Returns `Ok(true)` once the
/// decision is settled — either the block was consumed or the stream clearly does
/// not start with one — and `Ok(false)` while a partial header is still arriving.
fn strip_http_prologue(buf: &mut BytesMut) -> Result<bool, VpnError> {
    if buf.len() < 5 {
        return Ok(false); // not enough to tell yet
    }
    if !buf.starts_with(b"HTTP/") {
        return Ok(true); // framed data from the first byte
    }
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4) else {
        if buf.len() > 8 * 1024 {
            return Err(VpnError::Protocol(
                "FortiGate tunnel-upgrade header block exceeded size guard".into(),
            ));
        }
        return Ok(false); // header block still in flight
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let status = head.lines().next().unwrap_or("");
    if !status.contains("200") {
        return Err(VpnError::Tls(format!(
            "unexpected tunnel-upgrade response: {status}"
        )));
    }
    let _ = buf.split_to(end); // keep only the framed bytes that followed
    Ok(true)
}

/// Buffered PPP frame reader/writer over the hijacked TLS stream.
struct PppIo<'a, S> {
    stream: &'a mut S,
    buf: BytesMut,
    /// Whether the optional HTTP prologue has been dealt with.
    http_checked: bool,
}

impl<'a, S: AsyncRead + AsyncWrite + Unpin> PppIo<'a, S> {
    fn new(stream: &'a mut S, prime: BytesMut) -> Self {
        Self { stream, buf: prime, http_checked: false }
    }

    async fn send(&mut self, proto: u16, payload: &[u8]) -> Result<(), VpnError> {
        self.stream.write_all(&encode_ppp(proto, payload)).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Next frame, or `Ok(None)` on timeout. A partial frame stays buffered.
    async fn recv(&mut self, timeout: Duration) -> Result<Option<(u16, Vec<u8>)>, VpnError> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.http_checked {
                self.http_checked = strip_http_prologue(&mut self.buf)?;
            }
            if self.http_checked {
                if let Some(frame) = try_decode_ppp(&mut self.buf)? {
                    return Ok(Some(frame));
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            let n = match tokio::time::timeout(left, self.stream.read_buf(&mut self.buf)).await {
                Ok(Ok(n)) => n,
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => 0,
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => return Ok(None),
            };
            if n == 0 {
                return Err(VpnError::Tls(
                    "gateway closed the tunnel during PPP negotiation".into(),
                ));
            }
        }
    }
}

/// Run LCP then IPCP over the hijacked tunnel stream.
///
/// Returns the negotiated session plus any IPv4 data frames the gateway sent
/// while we were still negotiating, already re-framed so the forwarding loop's
/// decoder sees them as ordinary inbound frames ("prime" buffer). Dropping them
/// would silently lose the first packets of the session.
pub async fn negotiate<S>(stream: &mut S, prime: BytesMut) -> Result<(PppSession, BytesMut), VpnError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let magic = gen_magic();
    let mut io = PppIo::new(stream, prime);
    let mut early = BytesMut::new();
    let started = Instant::now();

    // ---- LCP ----
    let mut lcp_opts: Vec<(u8, Vec<u8>)> = vec![
        (LCP_OPT_MRU, DEFAULT_MRU.to_be_bytes().to_vec()),
        (LCP_OPT_MAGIC, magic.to_be_bytes().to_vec()),
    ];
    let mut lcp_id: u8 = 1;
    let mut mru = DEFAULT_MRU;
    let (mut ours_acked, mut theirs_acked) = (false, false);
    let mut last_tx = Instant::now();
    io.send(PPP_LCP, &build_cp(CODE_CONF_REQ, lcp_id, &build_options(&lcp_opts))).await?;

    let mut rounds = 0u32;
    while !(ours_acked && theirs_acked) {
        rounds += 1;
        if rounds > MAX_ROUNDS || started.elapsed() > NEGOTIATE_DEADLINE {
            return Err(VpnError::Protocol("LCP negotiation did not converge".into()));
        }
        let Some((proto, payload)) = io.recv(RECV_TIMEOUT).await? else {
            // Nothing arrived: retransmit our Configure-Request (RFC 1661 restart).
            if last_tx.elapsed() >= RESTART_INTERVAL && !ours_acked {
                io.send(PPP_LCP, &build_cp(CODE_CONF_REQ, lcp_id, &build_options(&lcp_opts)))
                    .await?;
                last_tx = Instant::now();
            }
            continue;
        };
        match proto {
            PPP_IPV4 => stage_early(&mut early, &payload),
            PPP_PAP | PPP_CHAP => {
                return Err(VpnError::AuthFailed(
                    "gateway requested PPP link authentication (PAP/CHAP), which is not supported \
                     — FortiGate normally authenticates over HTTPS before the tunnel"
                        .into(),
                ));
            }
            PPP_LCP => {
                let pkt = parse_cp(&payload)?;
                match pkt.code {
                    CODE_CONF_REQ => {
                        let peer_opts = parse_options(&pkt.data);
                        if let Some((_, v)) = peer_opts.iter().find(|(t, _)| *t == LCP_OPT_AUTH) {
                            return Err(VpnError::AuthFailed(format!(
                                "gateway demands PPP link authentication (LCP auth option 0x{}), \
                                 unsupported",
                                v.iter().map(|b| format!("{b:02x}")).collect::<String>()
                            )));
                        }
                        // Reject anything outside the set we actually implement,
                        // per RFC 1661; ack the rest.
                        let unknown: Vec<(u8, Vec<u8>)> = peer_opts
                            .iter()
                            .filter(|(t, _)| !matches!(*t, LCP_OPT_MRU | LCP_OPT_MAGIC))
                            .cloned()
                            .collect();
                        if unknown.is_empty() {
                            io.send(PPP_LCP, &build_cp(CODE_CONF_ACK, pkt.id, &pkt.data)).await?;
                            theirs_acked = true;
                        } else {
                            tracing::debug!(count = unknown.len(), "rejecting unknown LCP options");
                            io.send(
                                PPP_LCP,
                                &build_cp(CODE_CONF_REJ, pkt.id, &build_options(&unknown)),
                            )
                            .await?;
                        }
                    }
                    CODE_CONF_ACK if pkt.id == lcp_id => {
                        for (t, v) in parse_options(&pkt.data) {
                            if t == LCP_OPT_MRU && v.len() == 2 {
                                mru = u16::from_be_bytes([v[0], v[1]]);
                            }
                        }
                        ours_acked = true;
                    }
                    CODE_CONF_NAK => {
                        // Adopt the peer's counter-proposal for options we sent.
                        for (t, v) in parse_options(&pkt.data) {
                            if let Some(slot) = lcp_opts.iter_mut().find(|(ot, _)| *ot == t) {
                                slot.1 = v;
                            }
                        }
                        lcp_id = lcp_id.wrapping_add(1);
                        io.send(PPP_LCP, &build_cp(CODE_CONF_REQ, lcp_id, &build_options(&lcp_opts)))
                            .await?;
                        last_tx = Instant::now();
                    }
                    CODE_CONF_REJ => {
                        let rejected: Vec<u8> =
                            parse_options(&pkt.data).into_iter().map(|(t, _)| t).collect();
                        lcp_opts.retain(|(t, _)| !rejected.contains(t));
                        lcp_id = lcp_id.wrapping_add(1);
                        io.send(PPP_LCP, &build_cp(CODE_CONF_REQ, lcp_id, &build_options(&lcp_opts)))
                            .await?;
                        last_tx = Instant::now();
                    }
                    CODE_ECHO_REQ => {
                        let f = echo_reply(magic, &pkt);
                        io.stream.write_all(&f).await?;
                        io.stream.flush().await?;
                    }
                    CODE_TERM_REQ => {
                        io.send(PPP_LCP, &build_cp(CODE_TERM_ACK, pkt.id, &[])).await?;
                        return Err(VpnError::Protocol(
                            "gateway terminated the PPP link during LCP".into(),
                        ));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    tracing::info!(mru, "FortiGate PPP: LCP up");

    // ---- IPCP ----
    let zero = vec![0u8; 4];
    let mut ipcp_opts: Vec<(u8, Vec<u8>)> = vec![
        (IPCP_OPT_ADDR, zero.clone()),
        (IPCP_OPT_DNS1, zero.clone()),
        (IPCP_OPT_DNS2, zero),
    ];
    let mut ipcp_id: u8 = 1;
    let (mut ip_ours, mut ip_theirs) = (false, false);
    let mut address: Option<Ipv4Addr> = None;
    let mut peer: Option<Ipv4Addr> = None;
    let mut dns: Vec<Ipv4Addr> = Vec::new();
    last_tx = Instant::now();
    io.send(PPP_IPCP, &build_cp(CODE_CONF_REQ, ipcp_id, &build_options(&ipcp_opts))).await?;

    rounds = 0;
    while !(ip_ours && ip_theirs) {
        rounds += 1;
        if rounds > MAX_ROUNDS || started.elapsed() > NEGOTIATE_DEADLINE {
            return Err(VpnError::Protocol("IPCP negotiation did not converge".into()));
        }
        let Some((proto, payload)) = io.recv(RECV_TIMEOUT).await? else {
            if last_tx.elapsed() >= RESTART_INTERVAL && !ip_ours {
                io.send(PPP_IPCP, &build_cp(CODE_CONF_REQ, ipcp_id, &build_options(&ipcp_opts)))
                    .await?;
                last_tx = Instant::now();
            }
            continue;
        };
        match proto {
            PPP_IPV4 => stage_early(&mut early, &payload),
            PPP_LCP => {
                let pkt = parse_cp(&payload)?;
                match pkt.code {
                    CODE_ECHO_REQ => {
                        let f = echo_reply(magic, &pkt);
                        io.stream.write_all(&f).await?;
                        io.stream.flush().await?;
                    }
                    CODE_CONF_REQ => {
                        io.send(PPP_LCP, &build_cp(CODE_CONF_ACK, pkt.id, &pkt.data)).await?;
                    }
                    CODE_TERM_REQ => {
                        io.send(PPP_LCP, &build_cp(CODE_TERM_ACK, pkt.id, &[])).await?;
                        return Err(VpnError::Protocol(
                            "gateway terminated the PPP link during IPCP".into(),
                        ));
                    }
                    _ => {}
                }
            }
            PPP_IPCP => {
                let pkt = parse_cp(&payload)?;
                match pkt.code {
                    CODE_CONF_REQ => {
                        for (t, v) in parse_options(&pkt.data) {
                            if t == IPCP_OPT_ADDR {
                                peer = opt_ipv4(&v);
                            }
                        }
                        io.send(PPP_IPCP, &build_cp(CODE_CONF_ACK, pkt.id, &pkt.data)).await?;
                        ip_theirs = true;
                    }
                    CODE_CONF_ACK if pkt.id == ipcp_id => {
                        harvest_ipcp(&parse_options(&pkt.data), &mut address, &mut dns);
                        ip_ours = true;
                    }
                    CODE_CONF_NAK => {
                        // The gateway's Nak carries the real assigned address and
                        // DNS servers; echo them back verbatim to close the loop.
                        let naked = parse_options(&pkt.data);
                        harvest_ipcp(&naked, &mut address, &mut dns);
                        for (t, v) in naked {
                            match ipcp_opts.iter_mut().find(|(ot, _)| *ot == t) {
                                Some(slot) => slot.1 = v,
                                None => ipcp_opts.push((t, v)),
                            }
                        }
                        ipcp_id = ipcp_id.wrapping_add(1);
                        io.send(
                            PPP_IPCP,
                            &build_cp(CODE_CONF_REQ, ipcp_id, &build_options(&ipcp_opts)),
                        )
                        .await?;
                        last_tx = Instant::now();
                    }
                    CODE_CONF_REJ => {
                        let rejected: Vec<u8> =
                            parse_options(&pkt.data).into_iter().map(|(t, _)| t).collect();
                        ipcp_opts.retain(|(t, _)| !rejected.contains(t));
                        ipcp_id = ipcp_id.wrapping_add(1);
                        io.send(
                            PPP_IPCP,
                            &build_cp(CODE_CONF_REQ, ipcp_id, &build_options(&ipcp_opts)),
                        )
                        .await?;
                        last_tx = Instant::now();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let address = address.ok_or_else(|| {
        VpnError::Protocol("IPCP completed without assigning an IPv4 address".into())
    })?;
    tracing::info!(
        %address,
        peer = ?peer,
        dns_count = dns.len(),
        mru,
        "FortiGate PPP: IPCP up"
    );

    let leftover = std::mem::take(&mut io.buf);
    let mut prime_out = early;
    prime_out.extend_from_slice(&leftover);
    Ok((PppSession { address, peer, dns, mtu: mru, magic }, prime_out))
}

/// Pull the assigned address and DNS servers out of an IPCP option list.
fn harvest_ipcp(opts: &[(u8, Vec<u8>)], address: &mut Option<Ipv4Addr>, dns: &mut Vec<Ipv4Addr>) {
    for (t, v) in opts {
        match *t {
            IPCP_OPT_ADDR => {
                if let Some(a) = opt_ipv4(v) {
                    if !a.is_unspecified() {
                        *address = Some(a);
                    }
                }
            }
            IPCP_OPT_DNS1 | IPCP_OPT_DNS2 => {
                if let Some(a) = opt_ipv4(v) {
                    if !a.is_unspecified() && !dns.contains(&a) {
                        dns.push(a);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Re-frame an IPv4 packet that arrived mid-negotiation and stash it in the
/// prime buffer, so the forwarding loop's decoder sees it as a normal inbound
/// frame instead of losing it. Off the hot path: the gateway only sends traffic
/// this early if something on the inside is already talking to our address.
fn stage_early(dst: &mut BytesMut, payload: &[u8]) {
    let mut tmp = Vec::with_capacity(framing::FORTI_HEADER_LEN + 2 + payload.len());
    encode_ppp_append(PPP_IPV4, payload, &mut tmp);
    dst.extend_from_slice(&tmp);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp_round_trips() {
        let opts = build_options(&[(LCP_OPT_MRU, 1354u16.to_be_bytes().to_vec())]);
        let pkt = build_cp(CODE_CONF_REQ, 7, &opts);
        assert_eq!(pkt, vec![1, 7, 0, 8, 1, 4, 0x05, 0x4a]);
        let back = parse_cp(&pkt).unwrap();
        assert_eq!(back.code, CODE_CONF_REQ);
        assert_eq!(back.id, 7);
        assert_eq!(parse_options(&back.data), vec![(1u8, vec![0x05, 0x4a])]);
    }

    #[test]
    fn parse_cp_rejects_truncated_and_overlong() {
        assert!(parse_cp(&[1, 2, 0]).is_err());
        // Declares 40 bytes but only 8 present.
        assert!(parse_cp(&[1, 1, 0, 40, 1, 4, 0, 0]).is_err());
        // Declares less than the header.
        assert!(parse_cp(&[1, 1, 0, 2, 0, 0]).is_err());
    }

    #[test]
    fn parse_options_stops_at_malformed_entry() {
        // Valid MRU, then an option claiming length 9 with only 2 bytes left.
        let b = [1u8, 4, 0x05, 0x4a, 5, 9, 0, 0];
        assert_eq!(parse_options(&b), vec![(1u8, vec![0x05, 0x4a])]);
        // Zero/one-byte length must not loop forever.
        assert_eq!(parse_options(&[7u8, 0, 7, 1]), vec![]);
    }

    #[test]
    fn encodes_the_exact_bytes_observed_on_the_wire() {
        // The LCP Conf-Req this client sends first, verified against a live
        // FortiOS 7.x gateway: total=0x0016, magic 0x5050, body=0x0010,
        // proto 0xc021, then Conf-Req id=1 with MRU 1354 + Magic.
        let opts = build_options(&[
            (LCP_OPT_MRU, DEFAULT_MRU.to_be_bytes().to_vec()),
            (LCP_OPT_MAGIC, 0xdeadbeefu32.to_be_bytes().to_vec()),
        ]);
        let f = encode_ppp(PPP_LCP, &build_cp(CODE_CONF_REQ, 1, &opts));
        assert_eq!(
            f,
            vec![
                0x00, 0x16, 0x50, 0x50, 0x00, 0x10, // envelope
                0xc0, 0x21, // PPP proto = LCP
                0x01, 0x01, 0x00, 0x0e, // Conf-Req id=1 len=14
                0x01, 0x04, 0x05, 0x4a, // MRU 1354
                0x05, 0x06, 0xde, 0xad, 0xbe, 0xef, // Magic
            ]
        );
    }

    #[test]
    fn decodes_the_gateway_reply_captured_on_the_wire() {
        // Real bytes from vpn2 FortiOS: server Conf-Req (magic only) then the
        // Conf-Ack of our request, coalesced in one TLS read.
        let raw: Vec<u8> = vec![
            0x00, 0x12, 0x50, 0x50, 0x00, 0x0c, 0xc0, 0x21, 0x01, 0x01, 0x00, 0x0a, 0x05, 0x06,
            0x10, 0x9b, 0x22, 0x33, 0x00, 0x16, 0x50, 0x50, 0x00, 0x10, 0xc0, 0x21, 0x02, 0x01,
            0x00, 0x0e, 0x01, 0x04, 0x05, 0x4a, 0x05, 0x06, 0xde, 0xad, 0xbe, 0xef,
        ];
        let mut buf = BytesMut::from(&raw[..]);

        let (proto, payload) = try_decode_ppp(&mut buf).unwrap().unwrap();
        assert_eq!(proto, PPP_LCP);
        let pkt = parse_cp(&payload).unwrap();
        assert_eq!(pkt.code, CODE_CONF_REQ);
        assert_eq!(parse_options(&pkt.data), vec![(LCP_OPT_MAGIC, vec![0x10, 0x9b, 0x22, 0x33])]);

        let (proto, payload) = try_decode_ppp(&mut buf).unwrap().unwrap();
        assert_eq!(proto, PPP_LCP);
        let pkt = parse_cp(&payload).unwrap();
        assert_eq!(pkt.code, CODE_CONF_ACK);
        assert_eq!(
            parse_options(&pkt.data),
            vec![(LCP_OPT_MRU, vec![0x05, 0x4a]), (LCP_OPT_MAGIC, vec![0xde, 0xad, 0xbe, 0xef])]
        );
        assert!(buf.is_empty());
    }

    #[test]
    fn split_ppp_tolerates_hdlc_prefix() {
        assert_eq!(split_ppp(&[0xc0, 0x21, 1, 2]), Some((PPP_LCP, &[1u8, 2][..])));
        assert_eq!(
            split_ppp(&[0xff, 0x03, 0xc0, 0x21, 1, 2]),
            Some((PPP_LCP, &[1u8, 2][..]))
        );
        assert_eq!(split_ppp(&[0x00]), None);
    }

    #[test]
    fn ipv4_payload_round_trips() {
        let ip = vec![0x45, 0x00, 0x00, 0x1c, 0xab, 0xcd];
        let mut buf = BytesMut::from(&encode_ppp(PPP_IPV4, &ip)[..]);
        let (proto, payload) = try_decode_ppp(&mut buf).unwrap().unwrap();
        assert_eq!(proto, PPP_IPV4);
        assert_eq!(payload, ip);
        assert!(buf.is_empty());
    }

    #[test]
    fn partial_frame_is_preserved_for_the_next_read() {
        let full = encode_ppp(PPP_IPV4, &[1, 2, 3, 4]);
        let mut buf = BytesMut::from(&full[..full.len() - 2]);
        assert!(try_decode_ppp(&mut buf).unwrap().is_none());
        buf.extend_from_slice(&full[full.len() - 2..]);
        assert_eq!(try_decode_ppp(&mut buf).unwrap().unwrap().1, vec![1, 2, 3, 4]);
    }

    #[test]
    fn echo_reply_carries_our_magic_then_peer_data() {
        let req = CpPacket { code: CODE_ECHO_REQ, id: 9, data: vec![0x11, 0x22, 0x33, 0x44, 0xAA] };
        let f = echo_reply(0xdeadbeef, &req);
        // envelope(6) + proto(2) + cp header(4) + magic(4) + trailing data(1)
        assert_eq!(f.len(), 17);
        assert_eq!(&f[6..8], &[0xc0, 0x21]);
        assert_eq!(f[8], CODE_ECHO_REP);
        assert_eq!(f[9], 9, "reply must echo the request id");
        assert_eq!(&f[12..16], &[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(f[16], 0xAA);
    }

    #[test]
    fn harvest_ipcp_ignores_unspecified_and_dedupes() {
        let mut addr = None;
        let mut dns = Vec::new();
        harvest_ipcp(
            &[
                (IPCP_OPT_ADDR, vec![0, 0, 0, 0]),
                (IPCP_OPT_DNS1, vec![172, 29, 0, 25]),
                (IPCP_OPT_DNS2, vec![172, 29, 0, 25]),
            ],
            &mut addr,
            &mut dns,
        );
        assert_eq!(addr, None, "0.0.0.0 must not be taken as an assignment");
        assert_eq!(dns, vec![Ipv4Addr::new(172, 29, 0, 25)]);

        harvest_ipcp(&[(IPCP_OPT_ADDR, vec![10, 0, 3, 1])], &mut addr, &mut dns);
        assert_eq!(addr, Some(Ipv4Addr::new(10, 0, 3, 1)));
    }

    #[test]
    fn magic_is_never_zero() {
        for _ in 0..100 {
            assert_ne!(gen_magic(), 0);
        }
    }
}
