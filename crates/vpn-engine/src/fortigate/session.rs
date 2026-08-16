//! FortiGate tunnel upgrade (FG-SESS-01).
//!
//! `GET /remote/sslvpn-tunnel` over a fresh TLS connection, carrying the
//! `SVPNCOOKIE`. After the request the socket stops being HTTP and starts
//! carrying `0x5050`-framed PPP (see `framing.rs` + `ppp.rs`).
//!
//! **The gateway sends nothing here.** Verified against a live FortiOS 7.x
//! portal: after the request the server stays silent indefinitely, waiting for
//! the client's LCP Configure-Request. An earlier version of this function
//! probed for a response with a 5-second timeout and, on timeout, returned
//! success with an empty buffer — which made every failed upgrade look like a
//! healthy connect. There is nothing to probe for, so we do not: send the
//! request and hand the stream straight to the PPP layer, which tolerates a
//! leading `HTTP/1.1 200` block for the gateways that do emit one.
#![allow(dead_code)]

use bytes::BytesMut;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use super::http::USER_AGENT_TUNNEL;
use super::ppp::{self, PppSession};
use crate::error::VpnError;
use crate::tunnel::{connect_tls, CertTrust};

/// Build the `GET /remote/sslvpn-tunnel` upgrade request.
///
/// `Host: sslvpn` is the literal that openfortivpn and FortiClient send — the
/// gateway does not route on it, and matching the reference clients avoids
/// tripping any portal that inspects it.
pub fn build_tunnel_request(cookie: &str) -> String {
    format!(
        "GET /remote/sslvpn-tunnel HTTP/1.1\r\n\
         Host: sslvpn\r\n\
         User-Agent: {USER_AGENT_TUNNEL}\r\n\
         Cookie: SVPNCOOKIE={cookie}\r\n\
         \r\n"
    )
}

/// Open the packet tunnel and bring PPP up (FG-SESS-01 + FG-PPP-01).
///
/// Returns the live stream, the negotiated PPP session, and any IPv4 frames that
/// arrived during negotiation ("prime") which the forwarding loop must decode
/// before its first read. The cookie is never logged.
pub async fn open_tunnel(
    host: &str,
    port: u16,
    trust: &CertTrust,
    cookie: &str,
) -> Result<(TlsStream<TcpStream>, PppSession, BytesMut), VpnError> {
    let mut stream = connect_tls(host, port, trust).await?;
    stream
        .write_all(build_tunnel_request(cookie).as_bytes())
        .await?;
    stream.flush().await?;
    tracing::debug!("FortiGate tunnel upgrade sent — starting PPP negotiation");

    let (session, prime) = ppp::negotiate(&mut stream, BytesMut::new()).await?;
    Ok((stream, session, prime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_request_carries_cookie() {
        let r = build_tunnel_request("C00K1E");
        assert!(r.starts_with("GET /remote/sslvpn-tunnel HTTP/1.1\r\n"));
        assert!(r.contains("Cookie: SVPNCOOKIE=C00K1E"));
        assert!(r.contains("Host: sslvpn"));
        assert!(r.ends_with("\r\n\r\n"));
    }

    #[test]
    fn tunnel_user_agent_matches_the_reference_clients() {
        // The SV1 quirk (HTTP 405) only affects /remote/logincheck; OpenConnect
        // and FortiClient both send SV1 on the tunnel upgrade.
        assert!(build_tunnel_request("x").contains("User-Agent: Mozilla/5.0 SV1"));
    }
}
