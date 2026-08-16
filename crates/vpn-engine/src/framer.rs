//! Protocol-agnostic tunnel framing (CP-TUN-01). The forwarding loop
//! (`forward::run_forwarding`) drives bytes over TLS without knowing whether the
//! wire protocol is Cisco CSTP (v0.1) or Check Point SLIM (v0.2). Both implement
//! [`TunnelFramer`]: `encode_data`, `encode_keepalive`, and a buffered,
//! cancel-safe `try_decode` that yields a classified [`FrameEvent`].
//!
//! Phase 6 swaps `run_forwarding` onto `Box<dyn TunnelFramer>`; this phase
//! delivers and unit-tests the trait plus both implementations.
#![allow(dead_code)]

use bytes::BytesMut;

use crate::checkpoint::framing::{self, SlimPacket};
use crate::error::VpnError;
use crate::fortigate::ppp;
use crate::tunnel::{self, CstpType};

/// The protocol-agnostic result of decoding one inbound frame — what the forward
/// loop should do next. Any protocol-specific reply is already encoded into
/// `Reply` bytes, so the loop never branches on protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameEvent {
    /// A data (IP) payload — write it to the TUN device.
    Data(Vec<u8>),
    /// The peer requires a control reply — write these ready-made bytes back over
    /// TLS (e.g. a CSTP `DpdResp` answering a `DpdOut`). SLIM never uses this.
    Reply(Vec<u8>),
    /// A liveness/keepalive frame — no action beyond noting the peer is alive.
    Ignore,
    /// The peer asked to tear down — end the forwarding loop.
    Disconnect,
}

/// Encode + decode tunnel frames for one wire protocol. Decoding is buffered and
/// cancel-safe: `try_decode` returns `Ok(None)` until a whole frame is present and
/// leaves any partial frame in `buf` for the next read.
pub trait TunnelFramer: Send {
    /// Frame a data (IP) payload for transmission.
    fn encode_data(&self, payload: &[u8]) -> Vec<u8>;
    /// Append a framed data payload to a caller-owned buffer (does NOT clear it).
    /// This is the hot-path encoder: the TUN->TLS loop reuses one buffer across
    /// the connection (no per-packet `Vec`) AND appends several packets into it to
    /// coalesce a batch into a single TLS write. Frames are length-prefixed, so
    /// concatenated frames decode cleanly on the peer. The default delegates to
    /// [`encode_data`](Self::encode_data); protocol framers override it to append
    /// the header + payload directly with no intermediate allocation.
    fn encode_data_append(&self, payload: &[u8], out: &mut Vec<u8>) {
        out.extend_from_slice(&self.encode_data(payload));
    }
    /// Convenience: frame one payload into a freshly-cleared buffer.
    fn encode_data_into(&self, payload: &[u8], out: &mut Vec<u8>) {
        out.clear();
        self.encode_data_append(payload, out);
    }
    /// Build a client-initiated keepalive/liveness frame.
    fn encode_keepalive(&self) -> Vec<u8>;
    /// Optional in-tunnel frame to send on a polite client shutdown. CSTP sends a
    /// `Disconnect`; SLIM sends nothing in-tunnel (teardown is a CCC `Signout` on
    /// the auth channel — RESEARCH §5). `None` = send nothing.
    fn encode_shutdown(&self) -> Option<Vec<u8>> {
        None
    }
    /// Try to decode one frame from the front of `buf`, classifying it into a
    /// [`FrameEvent`]. `Ok(None)` = need more bytes. `Err` = malformed frame.
    fn try_decode(&mut self, buf: &mut BytesMut) -> Result<Option<FrameEvent>, VpnError>;
}

// ---------------------------------------------------------------------------
// Cisco CSTP (v0.1)
// ---------------------------------------------------------------------------

/// CSTP framer — wraps the v0.1 `tunnel` codec behind [`TunnelFramer`].
#[derive(Debug, Default, Clone, Copy)]
pub struct CstpTunnelFramer;

impl TunnelFramer for CstpTunnelFramer {
    fn encode_data(&self, payload: &[u8]) -> Vec<u8> {
        tunnel::CstpFramer::encode_data(payload)
    }

    fn encode_data_append(&self, payload: &[u8], out: &mut Vec<u8>) {
        let header = tunnel::write_header(CstpType::Data, payload.len());
        out.reserve(header.len() + payload.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(payload);
    }

    fn encode_keepalive(&self) -> Vec<u8> {
        // Client liveness tick = a CSTP DpdOut control frame (no payload).
        tunnel::write_header(CstpType::DpdOut, 0).to_vec()
    }

    fn encode_shutdown(&self) -> Option<Vec<u8>> {
        // Polite CSTP teardown: an empty Disconnect frame.
        Some(tunnel::write_header(CstpType::Disconnect, 0).to_vec())
    }

    fn try_decode(&mut self, buf: &mut BytesMut) -> Result<Option<FrameEvent>, VpnError> {
        let Some(packet) = tunnel::try_decode_cstp(buf)? else {
            return Ok(None);
        };
        let event = match packet.packet_type {
            CstpType::Data => FrameEvent::Data(packet.payload),
            // Server DPD request -> answer with an empty DpdResp frame.
            CstpType::DpdOut => {
                FrameEvent::Reply(tunnel::write_header(CstpType::DpdResp, 0).to_vec())
            }
            CstpType::DpdResp | CstpType::Keepalive | CstpType::Compressed => FrameEvent::Ignore,
            CstpType::Disconnect | CstpType::TermServer => FrameEvent::Disconnect,
        };
        Ok(Some(event))
    }
}

// ---------------------------------------------------------------------------
// Check Point SLIM (v0.2)
// ---------------------------------------------------------------------------

/// SLIM framer — wraps the `checkpoint::framing` codec behind [`TunnelFramer`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SlimTunnelFramer;

impl TunnelFramer for SlimTunnelFramer {
    fn encode_data(&self, payload: &[u8]) -> Vec<u8> {
        framing::encode_data(payload)
    }

    fn encode_data_append(&self, payload: &[u8], out: &mut Vec<u8>) {
        framing::encode_data_append(payload, out);
    }

    fn encode_keepalive(&self) -> Vec<u8> {
        framing::encode_keepalive()
    }

    fn try_decode(&mut self, buf: &mut BytesMut) -> Result<Option<FrameEvent>, VpnError> {
        let Some(packet) = framing::try_decode_slim(buf)? else {
            return Ok(None);
        };
        let event = match packet {
            SlimPacket::Data(payload) => FrameEvent::Data(payload),
            // Control frames dispatch on the S-expression object name.
            SlimPacket::Control(tree) => match tree.name() {
                Some("disconnect") => FrameEvent::Disconnect,
                // keepalive (and any other control) -> liveness only. SLIM does
                // not echo server keepalives (RESEARCH §4).
                _ => FrameEvent::Ignore,
            },
        };
        Ok(Some(event))
    }
}

// ---------------------------------------------------------------------------
// FortiGate SSL VPN (v0.3)
// ---------------------------------------------------------------------------

/// FortiGate framer — `0x5050` envelope carrying PPP.
///
/// Data frames are PPP protocol `0x0021` (raw IPv4). Unlike the earlier
/// "raw IP, no keepalive" assumption, PPP gives us a real liveness probe: the
/// keepalive is an **LCP Echo-Request**, and the peer's Echo-Request is answered
/// with an Echo-Reply. That matters because FortiGate advertises an
/// `<idle-timeout>` (300 s on the reference gateway) and silently drops a tunnel
/// that goes quiet. Shutdown sends an LCP Terminate-Request.
///
/// `magic` is the LCP magic number agreed during negotiation; RFC 1661 requires
/// it in every Echo frame.
#[derive(Debug)]
pub struct FortinetPppFramer {
    magic: u32,
    /// Echo-Request identifier, bumped per probe so replies can be correlated.
    echo_id: std::cell::Cell<u8>,
}

impl FortinetPppFramer {
    pub fn new(magic: u32) -> Self {
        Self { magic, echo_id: std::cell::Cell::new(0) }
    }
}

impl TunnelFramer for FortinetPppFramer {
    fn encode_data(&self, payload: &[u8]) -> Vec<u8> {
        ppp::encode_ppp(ppp::PPP_IPV4, payload)
    }

    fn encode_data_append(&self, payload: &[u8], out: &mut Vec<u8>) {
        ppp::encode_ppp_append(ppp::PPP_IPV4, payload, out);
    }

    fn encode_keepalive(&self) -> Vec<u8> {
        let id = self.echo_id.get().wrapping_add(1);
        self.echo_id.set(id);
        ppp::echo_request(self.magic, id)
    }

    fn encode_shutdown(&self) -> Option<Vec<u8>> {
        Some(ppp::terminate_request(0))
    }

    fn try_decode(&mut self, buf: &mut BytesMut) -> Result<Option<FrameEvent>, VpnError> {
        let Some((proto, payload)) = ppp::try_decode_ppp(buf)? else {
            return Ok(None);
        };
        if proto == ppp::PPP_IPV4 {
            return Ok(Some(FrameEvent::Data(payload)));
        }
        // Control protocols. A malformed control packet inside a well-formed
        // envelope is logged and skipped rather than tearing the tunnel down —
        // data forwarding does not depend on it.
        let pkt = match ppp::parse_cp(&payload) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(proto = format!("0x{proto:04x}"), error = %e,
                    "ignoring malformed PPP control packet");
                return Ok(Some(FrameEvent::Ignore));
            }
        };
        let event = match (proto, pkt.code) {
            (ppp::PPP_LCP, ppp::CODE_ECHO_REQ) => {
                FrameEvent::Reply(ppp::echo_reply(self.magic, &pkt))
            }
            // The peer re-opening negotiation mid-session: acknowledge so the
            // link stays up instead of stalling.
            (ppp::PPP_LCP, ppp::CODE_CONF_REQ) | (ppp::PPP_IPCP, ppp::CODE_CONF_REQ) => {
                FrameEvent::Reply(ppp::encode_ppp(
                    proto,
                    &ppp::build_cp(ppp::CODE_CONF_ACK, pkt.id, &pkt.data),
                ))
            }
            (ppp::PPP_LCP, ppp::CODE_TERM_REQ) => {
                tracing::info!("gateway sent LCP Terminate-Request");
                FrameEvent::Disconnect
            }
            _ => FrameEvent::Ignore,
        };
        Ok(Some(event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- CSTP ---

    #[test]
    fn encode_data_into_matches_encode_data() {
        let payload = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x45];
        for (name, framer) in [
            ("cstp", &CstpTunnelFramer as &dyn TunnelFramer),
            ("slim", &SlimTunnelFramer as &dyn TunnelFramer),
        ] {
            let owned = framer.encode_data(&payload);
            // Reuse a pre-dirtied buffer to prove encode_data_into clears it.
            let mut buf = vec![0xFF; 3];
            framer.encode_data_into(&payload, &mut buf);
            assert_eq!(owned, buf, "{name}: encode_data_into diverged from encode_data");
        }
    }

    #[test]
    fn coalesced_batch_decodes_as_sequence() {
        // A TX batch is several frames appended into one buffer; the peer must
        // decode them back as the same ordered sequence of packets.
        let a = [0x11u8, 0x22, 0x33];
        let b = [0x44u8, 0x55];
        for (name, mut framer) in [
            ("cstp", Box::new(CstpTunnelFramer) as Box<dyn TunnelFramer>),
            ("slim", Box::new(SlimTunnelFramer) as Box<dyn TunnelFramer>),
        ] {
            let mut batch = Vec::new();
            framer.encode_data_append(&a, &mut batch);
            framer.encode_data_append(&b, &mut batch);
            let mut buf = BytesMut::from(&batch[..]);
            assert_eq!(
                framer.try_decode(&mut buf).unwrap(),
                Some(FrameEvent::Data(a.to_vec())),
                "{name}: first frame"
            );
            assert_eq!(
                framer.try_decode(&mut buf).unwrap(),
                Some(FrameEvent::Data(b.to_vec())),
                "{name}: second frame"
            );
            assert_eq!(framer.try_decode(&mut buf).unwrap(), None, "{name}: drained");
        }
    }

    #[test]
    fn cstp_data_round_trips_through_framer() {
        let mut f = CstpTunnelFramer;
        let frame = f.encode_data(&[0x11, 0x22, 0x33]);
        let mut buf = BytesMut::from(&frame[..]);
        assert_eq!(
            f.try_decode(&mut buf).unwrap(),
            Some(FrameEvent::Data(vec![0x11, 0x22, 0x33]))
        );
    }

    #[test]
    fn cstp_server_dpd_out_yields_reply() {
        let mut f = CstpTunnelFramer;
        let dpd_out = tunnel::write_header(CstpType::DpdOut, 0);
        let mut buf = BytesMut::from(&dpd_out[..]);
        match f.try_decode(&mut buf).unwrap() {
            Some(FrameEvent::Reply(bytes)) => {
                // The reply is a valid DpdResp frame.
                let (t, len) = tunnel::parse_header(bytes.as_slice().try_into().unwrap()).unwrap();
                assert_eq!(t, CstpType::DpdResp);
                assert_eq!(len, 0);
            }
            other => panic!("expected Reply, got {other:?}"),
        }
    }

    #[test]
    fn cstp_disconnect_yields_disconnect() {
        let mut f = CstpTunnelFramer;
        let frame = tunnel::write_header(CstpType::Disconnect, 0);
        let mut buf = BytesMut::from(&frame[..]);
        assert_eq!(
            f.try_decode(&mut buf).unwrap(),
            Some(FrameEvent::Disconnect)
        );
    }

    #[test]
    fn cstp_keepalive_encodes_dpd_out() {
        let f = CstpTunnelFramer;
        let frame = f.encode_keepalive();
        let (t, len) = tunnel::parse_header(frame.as_slice().try_into().unwrap()).unwrap();
        assert_eq!(t, CstpType::DpdOut);
        assert_eq!(len, 0);
    }

    #[test]
    fn cstp_partial_frame_needs_more() {
        let mut f = CstpTunnelFramer;
        let mut buf = BytesMut::from(&[0x53, 0x54, 0x46][..]); // partial header
        assert_eq!(f.try_decode(&mut buf).unwrap(), None);
    }

    // --- SLIM ---

    #[test]
    fn slim_data_round_trips_through_framer() {
        let mut f = SlimTunnelFramer;
        let frame = f.encode_data(&[0xAB, 0xCD]);
        let mut buf = BytesMut::from(&frame[..]);
        assert_eq!(
            f.try_decode(&mut buf).unwrap(),
            Some(FrameEvent::Data(vec![0xAB, 0xCD]))
        );
    }

    #[test]
    fn slim_keepalive_is_ignored_on_decode() {
        let mut f = SlimTunnelFramer;
        let frame = f.encode_keepalive();
        let mut buf = BytesMut::from(&frame[..]);
        assert_eq!(f.try_decode(&mut buf).unwrap(), Some(FrameEvent::Ignore));
    }

    #[test]
    fn slim_disconnect_yields_disconnect() {
        let mut f = SlimTunnelFramer;
        let frame = framing::encode_control("(disconnect :code (0))");
        let mut buf = BytesMut::from(&frame[..]);
        assert_eq!(
            f.try_decode(&mut buf).unwrap(),
            Some(FrameEvent::Disconnect)
        );
    }

    #[test]
    fn slim_unknown_control_is_ignored() {
        let mut f = SlimTunnelFramer;
        let frame = framing::encode_control("(hello_reply :OM ( :ipaddr (10.0.0.10)))");
        let mut buf = BytesMut::from(&frame[..]);
        assert_eq!(f.try_decode(&mut buf).unwrap(), Some(FrameEvent::Ignore));
    }

    #[test]
    fn cstp_shutdown_is_disconnect_slim_is_none() {
        let cstp = CstpTunnelFramer.encode_shutdown().expect("CSTP sends disconnect");
        let (t, len) = tunnel::parse_header(cstp.as_slice().try_into().unwrap()).unwrap();
        assert_eq!(t, CstpType::Disconnect);
        assert_eq!(len, 0);
        assert_eq!(SlimTunnelFramer.encode_shutdown(), None);
    }

    #[test]
    fn slim_partial_frame_needs_more() {
        let mut f = SlimTunnelFramer;
        let mut buf = BytesMut::from(&[0x00, 0x00][..]); // partial header
        assert_eq!(f.try_decode(&mut buf).unwrap(), None);
    }

    // --- FortiGate (PPP) ---

    #[test]
    fn fortinet_data_round_trips_through_framer() {
        let mut f = FortinetPppFramer::new(0xdeadbeef);
        let frame = f.encode_data(&[0x45, 0x00, 0x11]);
        // Data must go out as PPP protocol 0x0021 inside the 0x5050 envelope.
        assert_eq!(&frame[..8], &[0x00, 0x0b, 0x50, 0x50, 0x00, 0x05, 0x00, 0x21]);
        let mut buf = BytesMut::from(&frame[..]);
        assert_eq!(
            f.try_decode(&mut buf).unwrap(),
            Some(FrameEvent::Data(vec![0x45, 0x00, 0x11]))
        );
    }

    #[test]
    fn fortinet_keepalive_is_an_lcp_echo_request() {
        let f = FortinetPppFramer::new(0xdeadbeef);
        let ka = f.encode_keepalive();
        assert_eq!(&ka[6..8], &[0xc0, 0x21], "keepalive must be LCP");
        assert_eq!(ka[8], ppp::CODE_ECHO_REQ);
        assert_eq!(&ka[12..16], &[0xde, 0xad, 0xbe, 0xef], "must carry our magic");
        // The id advances per probe.
        assert_eq!(ka[9], 1);
        assert_eq!(f.encode_keepalive()[9], 2);
    }

    #[test]
    fn fortinet_shutdown_is_an_lcp_terminate_request() {
        let s = FortinetPppFramer::new(1).encode_shutdown().expect("PPP sends Terminate-Request");
        assert_eq!(&s[6..8], &[0xc0, 0x21]);
        assert_eq!(s[8], ppp::CODE_TERM_REQ);
    }

    #[test]
    fn fortinet_peer_echo_request_gets_a_reply() {
        let mut f = FortinetPppFramer::new(0xdeadbeef);
        // Peer Echo-Request id=42 carrying its own magic.
        let req = ppp::encode_ppp(
            ppp::PPP_LCP,
            &ppp::build_cp(ppp::CODE_ECHO_REQ, 42, &[0x11, 0x22, 0x33, 0x44]),
        );
        let mut buf = BytesMut::from(&req[..]);
        let Some(FrameEvent::Reply(reply)) = f.try_decode(&mut buf).unwrap() else {
            panic!("an Echo-Request must produce a Reply");
        };
        assert_eq!(reply[8], ppp::CODE_ECHO_REP);
        assert_eq!(reply[9], 42, "reply echoes the request id");
        assert_eq!(&reply[12..16], &[0xde, 0xad, 0xbe, 0xef], "reply carries OUR magic");
    }

    #[test]
    fn fortinet_terminate_request_disconnects() {
        let mut f = FortinetPppFramer::new(1);
        let term = ppp::encode_ppp(ppp::PPP_LCP, &ppp::build_cp(ppp::CODE_TERM_REQ, 3, b"bye"));
        let mut buf = BytesMut::from(&term[..]);
        assert_eq!(f.try_decode(&mut buf).unwrap(), Some(FrameEvent::Disconnect));
    }

    #[test]
    fn fortinet_conf_req_is_acknowledged() {
        let mut f = FortinetPppFramer::new(1);
        let req = ppp::encode_ppp(ppp::PPP_IPCP, &ppp::build_cp(ppp::CODE_CONF_REQ, 5, &[3, 6, 10, 0, 0, 1]));
        let mut buf = BytesMut::from(&req[..]);
        let Some(FrameEvent::Reply(ack)) = f.try_decode(&mut buf).unwrap() else {
            panic!("a Conf-Req must be acknowledged");
        };
        assert_eq!(&ack[6..8], &[0x80, 0x21]);
        assert_eq!(ack[8], ppp::CODE_CONF_ACK);
        assert_eq!(ack[9], 5);
    }

    #[test]
    fn fortinet_unknown_control_is_ignored_not_fatal() {
        let mut f = FortinetPppFramer::new(1);
        // LCP Code-Reject: nothing to do, but it must not kill the tunnel.
        let pkt = ppp::encode_ppp(ppp::PPP_LCP, &ppp::build_cp(ppp::CODE_CODE_REJ, 1, &[0xAA]));
        let mut buf = BytesMut::from(&pkt[..]);
        assert_eq!(f.try_decode(&mut buf).unwrap(), Some(FrameEvent::Ignore));
    }

    #[test]
    fn fortinet_coalesced_batch_decodes_as_sequence() {
        let mut f = FortinetPppFramer::new(1);
        let mut batch = Vec::new();
        f.encode_data_append(&[0x11, 0x22], &mut batch);
        f.encode_data_append(&[0x33], &mut batch);
        let mut buf = BytesMut::from(&batch[..]);
        assert_eq!(f.try_decode(&mut buf).unwrap(), Some(FrameEvent::Data(vec![0x11, 0x22])));
        assert_eq!(f.try_decode(&mut buf).unwrap(), Some(FrameEvent::Data(vec![0x33])));
        assert_eq!(f.try_decode(&mut buf).unwrap(), None);
    }

    #[test]
    fn framers_are_trait_objects() {
        // Prove all three are usable behind the dyn trait the forward loop holds.
        let framers: Vec<Box<dyn TunnelFramer>> = vec![
            Box::new(CstpTunnelFramer),
            Box::new(SlimTunnelFramer),
            Box::new(FortinetPppFramer::new(0x12345678)),
        ];
        for f in framers {
            assert!(!f.encode_data(&[0x01]).is_empty());
            assert!(!f.encode_keepalive().is_empty());
        }
    }
}
