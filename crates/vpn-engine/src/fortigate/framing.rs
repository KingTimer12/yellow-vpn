//! FortiGate SSL VPN outer frame codec (FG-TUN-01). Every frame is a 6-byte
//! big-endian header followed by the body:
//!
//! ```text
//! offset 0  u16  total length  = 6 + body_len   (big-endian)
//! offset 2  u16  magic         = 0x5050 ("PP")   (big-endian)
//! offset 4  u16  body_len                        (big-endian)
//! offset 6  ..   body
//! ```
//!
//! This module handles ONLY the envelope. The body is a PPP frame (a big-endian
//! protocol field followed by that protocol's payload) — see `ppp.rs`, which owns
//! everything above this layer. Source: OpenConnect `PPP_ENCAP_FORTINET` in
//! `ppp.c`, cross-checked against a live FortiOS 7.x capture.
//!
//! Server frames are untrusted: decoding is bounded (max body) and never panics;
//! malformed input maps to `VpnError::Protocol`. The decoder is buffered and
//! cancel-safe (mirrors `tunnel::try_decode_cstp` / `checkpoint::framing`).
#![allow(dead_code)]

use bytes::{Buf, BytesMut};

use crate::error::VpnError;

/// Fixed FortiGate frame header length (BE u16 total + BE u16 magic + BE u16 len).
pub const FORTI_HEADER_LEN: usize = 6;

/// Frame magic — the ASCII bytes "PP" (0x50 0x50), big-endian u16.
pub const FORTI_MAGIC: u16 = 0x5050;

/// Largest body we will accept. The length fields are `u16`, so 65529 is the
/// arithmetic ceiling; we cap far lower because the negotiated MRU is ~1354 and
/// anything near the ceiling is a malformed or hostile header.
const MAX_FORTI_BODY: usize = 16 * 1024;

/// Encode one frame around `body` (which must already carry the PPP protocol
/// field). Returns `Err` if the body cannot be expressed in the u16 length
/// fields — a truncating cast here would silently corrupt the stream.
pub fn encode_frame(body: &[u8]) -> Result<Vec<u8>, VpnError> {
    let mut out = Vec::with_capacity(FORTI_HEADER_LEN + body.len());
    encode_frame_append(body, &mut out)?;
    Ok(out)
}

/// Append a frame to a caller-owned buffer, so the hot forwarding path can reuse
/// one allocation across packets AND coalesce several packets into one buffer
/// (frames are length-prefixed, so back-to-back frames decode cleanly).
/// Does NOT clear `out`.
pub fn encode_frame_append(body: &[u8], out: &mut Vec<u8>) -> Result<(), VpnError> {
    if body.len() > MAX_FORTI_BODY {
        return Err(VpnError::Protocol(format!(
            "FortiGate frame body {} exceeds cap {MAX_FORTI_BODY}",
            body.len()
        )));
    }
    let total = (FORTI_HEADER_LEN + body.len()) as u16;
    out.reserve(FORTI_HEADER_LEN + body.len());
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&FORTI_MAGIC.to_be_bytes());
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
    Ok(())
}

/// Try to decode one frame from the front of `buf`, consuming its bytes on
/// success and returning the body. `Ok(None)` when more bytes are still needed —
/// the caller keeps the buffer across reads, which makes the inbound read path
/// cancellation-safe (a partial frame is never lost when a sibling `select!` arm
/// wins). `Err` on a bad magic, a length disagreement, or an over-large body.
/// Pure — no I/O.
pub fn try_decode_frame(buf: &mut BytesMut) -> Result<Option<Vec<u8>>, VpnError> {
    if buf.len() < FORTI_HEADER_LEN {
        return Ok(None); // header not fully arrived yet
    }
    let total = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    let magic = u16::from_be_bytes([buf[2], buf[3]]);
    let body_len = u16::from_be_bytes([buf[4], buf[5]]) as usize;

    if magic != FORTI_MAGIC {
        return Err(VpnError::Protocol(format!(
            "bad FortiGate frame magic: 0x{magic:04x}"
        )));
    }
    // total MUST equal header + body; reject a self-inconsistent header before
    // trusting either length to size a read.
    if total != FORTI_HEADER_LEN + body_len {
        return Err(VpnError::Protocol(format!(
            "FortiGate length mismatch: total {total}, body {body_len}"
        )));
    }
    if body_len > MAX_FORTI_BODY {
        return Err(VpnError::Protocol(format!(
            "FortiGate frame body {body_len} exceeds cap"
        )));
    }
    if buf.len() < FORTI_HEADER_LEN + body_len {
        return Ok(None); // body not fully arrived yet — wait for more bytes
    }
    buf.advance(FORTI_HEADER_LEN); // drop the header
    Ok(Some(buf.split_to(body_len).to_vec())) // consume exactly the body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_header_is_big_endian() {
        let frame = encode_frame(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
        assert_eq!(&frame[0..2], &[0x00, 0x0A]); // total = 6 + 4 = 10 BE
        assert_eq!(&frame[2..4], &[0x50, 0x50]); // magic BE
        assert_eq!(&frame[4..6], &[0x00, 0x04]); // body_len = 4 BE
        assert_eq!(&frame[6..], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn body_round_trips() {
        let body = vec![0x00, 0x21, 0x45, 0x00, 0x00, 0x14]; // PPP IPv4 + IP header start
        let frame = encode_frame(&body).unwrap();
        let mut buf = BytesMut::from(&frame[..]);
        let out = try_decode_frame(&mut buf).unwrap().unwrap();
        assert_eq!(out, body);
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_waits_for_full_header_then_body() {
        let mut buf = BytesMut::new();
        // Partial header -> None, nothing consumed.
        buf.extend_from_slice(&[0x00, 0x0A, 0x50]);
        assert!(try_decode_frame(&mut buf).unwrap().is_none());
        assert_eq!(buf.len(), 3);
        // Full header declaring a 4-byte body, body absent -> None.
        buf.clear();
        buf.extend_from_slice(&[0x00, 0x0A, 0x50, 0x50, 0x00, 0x04]);
        assert!(try_decode_frame(&mut buf).unwrap().is_none());
        assert_eq!(buf.len(), FORTI_HEADER_LEN);
        // Body arrives -> full frame decoded and consumed.
        buf.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]);
        let out = try_decode_frame(&mut buf).unwrap().unwrap();
        assert_eq!(out, vec![0x01, 0x02, 0x03, 0x04]);
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_drains_two_coalesced_frames_and_keeps_partial() {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&encode_frame(&[0xAA]).unwrap());
        buf.extend_from_slice(&encode_frame(&[0xBB, 0xCC]).unwrap());
        buf.extend_from_slice(&[0x00]); // partial third frame
        assert_eq!(try_decode_frame(&mut buf).unwrap().unwrap(), vec![0xAA]);
        assert_eq!(try_decode_frame(&mut buf).unwrap().unwrap(), vec![0xBB, 0xCC]);
        // Partial third frame preserved (cancel-safety).
        assert!(try_decode_frame(&mut buf).unwrap().is_none());
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&[0x00, 0x06, 0xDE, 0xAD, 0x00, 0x00]);
        assert!(matches!(try_decode_frame(&mut buf), Err(VpnError::Protocol(_))));
    }

    #[test]
    fn length_mismatch_is_rejected() {
        let mut buf = BytesMut::new();
        // total says 10, body says 8 — inconsistent.
        buf.extend_from_slice(&[0x00, 0x0A, 0x50, 0x50, 0x00, 0x08]);
        assert!(matches!(try_decode_frame(&mut buf), Err(VpnError::Protocol(_))));
    }

    #[test]
    fn oversized_body_is_rejected_instead_of_truncated() {
        let big = vec![0u8; MAX_FORTI_BODY + 1];
        assert!(matches!(encode_frame(&big), Err(VpnError::Protocol(_))));
        let mut out = Vec::new();
        assert!(encode_frame_append(&big, &mut out).is_err());
        assert!(out.is_empty(), "a rejected frame must not partially write");
    }

    #[test]
    fn decode_rejects_declared_body_over_cap() {
        let mut buf = BytesMut::new();
        let body_len = (MAX_FORTI_BODY + 1) as u16;
        buf.extend_from_slice(&(body_len + 6).to_be_bytes());
        buf.extend_from_slice(&FORTI_MAGIC.to_be_bytes());
        buf.extend_from_slice(&body_len.to_be_bytes());
        assert!(matches!(try_decode_frame(&mut buf), Err(VpnError::Protocol(_))));
    }
}
