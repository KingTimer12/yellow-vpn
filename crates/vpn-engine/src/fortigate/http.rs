//! Minimal HTTP/1.1 client over the engine's TLS layer (FG-HTTP-01).
//!
//! The previous FortiGate code read HTTP responses by looping until EOF and
//! trusting `Connection: close`. Real FortiOS answers with
//! `Transfer-Encoding: chunked` (verified against FortiOS 7.x: every
//! `/remote/*` reply is chunked), so an EOF-driven reader either hangs or —
//! with a read timeout bolted on — throws away a response that already arrived
//! complete. This module delimits the body properly: chunked first,
//! `Content-Length` second, EOF only as the last resort.
//!
//! No `reqwest`: the tunnel socket gets hijacked after `GET
//! /remote/sslvpn-tunnel`, so the HTTP layer has to hand back a raw stream.
//! Deps are LOCKED, so this is a hand-rolled parser — bounded and panic-free,
//! because every byte here is untrusted server input.
#![allow(dead_code)]

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::error::VpnError;
use crate::tunnel::{connect_tls, CertTrust};

/// Client User-Agent. MUST NOT contain the substring `SV1` on `/remote/logincheck`
/// — some FortiOS versions answer HTTP 405 to that (openfortivpn issue #409).
pub const USER_AGENT: &str = "Mozilla/5.0";

/// User-Agent for the tunnel upgrade. OpenConnect uses `Mozilla/5.0 SV1` here and
/// the 405 quirk only affects the login POST, so the tunnel keeps the SV1 form
/// that FortiClient itself sends.
pub const USER_AGENT_TUNNEL: &str = "Mozilla/5.0 SV1";

/// Largest response we will buffer (256 KiB). A real reply is a few KiB; this
/// bounds a hostile or runaway server.
pub const RESPONSE_MAX: usize = 256 * 1024;

/// Upper bound on one request/response round-trip. Unlike the old per-read
/// timeout this is a genuine failure signal: the body is delimited, so hitting
/// this means the server really did stop talking mid-message.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// A parsed HTTP response.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    /// Header names are lowercased; values keep their original casing.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// First value for a header name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        let want = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == want)
            .map(|(_, v)| v.as_str())
    }

    /// Body as lossy UTF-8.
    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Extract a cookie value from the `Set-Cookie` headers.
    ///
    /// FortiGate clears a cookie by re-sending it with an EMPTY value and an
    /// expiry in 1984 — the login reply does exactly that to `SVPNCOOKIE` when a
    /// host-check is still pending. So an empty value (and the `0` sentinel) is a
    /// deletion, not a credential: skip both and keep the last usable value.
    pub fn cookie(&self, name: &str) -> Option<String> {
        let mut found = None;
        for (k, v) in &self.headers {
            if k != "set-cookie" {
                continue;
            }
            let Some(start) = v.find(&format!("{name}=")) else {
                continue;
            };
            // Guard against `SVPNCOOKIE` matching inside `SVPNTMPCOOKIE`: the
            // character before the match must not be part of a cookie name.
            if start > 0 {
                let prev = v.as_bytes()[start - 1];
                if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'-' {
                    continue;
                }
            }
            let rest = &v[start + name.len() + 1..];
            let end = rest.find([';', ',']).unwrap_or(rest.len());
            let val = rest[..end].trim();
            if !val.is_empty() && val != "0" {
                found = Some(val.to_string());
            }
        }
        found
    }
}

/// Build a request. `cookie` is the full `Cookie:` header value (e.g.
/// `SVPNCOOKIE=abc`). A `Some(body)` makes it a form POST.
pub fn build_request(
    method: &str,
    path: &str,
    host: &str,
    user_agent: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> String {
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         User-Agent: {user_agent}\r\n\
         Accept: */*\r\n"
    );
    if let Some(c) = cookie {
        req.push_str(&format!("Cookie: {c}\r\n"));
    }
    match body {
        Some(b) => {
            req.push_str("Content-Type: application/x-www-form-urlencoded\r\n");
            req.push_str(&format!("Content-Length: {}\r\n\r\n{b}", b.len()));
        }
        None => req.push_str("\r\n"),
    }
    req
}

/// Position just past the first `\r\n\r\n`, if the header block is complete.
fn header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Parse the status line + headers out of a complete header block.
fn parse_head(head: &str) -> Result<(u16, Vec<(String, String)>), VpnError> {
    let mut lines = head.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| VpnError::Protocol("empty HTTP response".into()))?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| {
            VpnError::Protocol(format!("malformed HTTP status line: {status_line:?}"))
        })?;
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Ok((status, headers))
}

/// Decode a chunked body. Returns `Ok(None)` while the terminating zero-size
/// chunk has not arrived yet, so the caller knows to read more.
fn decode_chunked(buf: &[u8]) -> Result<Option<Vec<u8>>, VpnError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        // Chunk-size line: hex digits, optional `;ext`, then CRLF.
        let Some(nl) = buf[pos..].windows(2).position(|w| w == b"\r\n") else {
            return Ok(None); // size line incomplete
        };
        let line = &buf[pos..pos + nl];
        let hex = line
            .split(|b| *b == b';')
            .next()
            .unwrap_or(line);
        let text = std::str::from_utf8(hex)
            .map_err(|_| VpnError::Protocol("non-UTF8 chunk size".into()))?
            .trim();
        let size = usize::from_str_radix(text, 16)
            .map_err(|_| VpnError::Protocol(format!("bad chunk size {text:?}")))?;
        pos += nl + 2;
        if size == 0 {
            return Ok(Some(out)); // trailers (if any) are ignored
        }
        if out.len() + size > RESPONSE_MAX {
            return Err(VpnError::Protocol("chunked body exceeded size guard".into()));
        }
        if buf.len() < pos + size + 2 {
            return Ok(None); // chunk data (or its trailing CRLF) still in flight
        }
        out.extend_from_slice(&buf[pos..pos + size]);
        pos += size + 2; // skip the chunk's CRLF
    }
}

/// Read one complete HTTP response. Delimits the body by chunked encoding,
/// then `Content-Length`, then EOF — in that order.
pub async fn read_response<S>(stream: &mut S) -> Result<HttpResponse, VpnError>
where
    S: AsyncRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];
    let mut head: Option<(usize, u16, Vec<(String, String)>)> = None;

    loop {
        // Parse the head as soon as it is complete, so we know how to delimit.
        if head.is_none() {
            if let Some(end) = header_end(&buf) {
                let text = String::from_utf8_lossy(&buf[..end]).into_owned();
                let (status, headers) = parse_head(&text)?;
                head = Some((end, status, headers));
            }
        }
        if let Some((end, status, headers)) = &head {
            let rest = &buf[*end..];
            let chunked = headers
                .iter()
                .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
            let clen: Option<usize> = headers
                .iter()
                .find(|(k, _)| k == "content-length")
                .and_then(|(_, v)| v.trim().parse().ok());

            if chunked {
                if let Some(body) = decode_chunked(rest)? {
                    return Ok(HttpResponse { status: *status, headers: headers.clone(), body });
                }
            } else if let Some(n) = clen {
                if n > RESPONSE_MAX {
                    return Err(VpnError::Protocol("Content-Length exceeds size guard".into()));
                }
                if rest.len() >= n {
                    return Ok(HttpResponse {
                        status: *status,
                        headers: headers.clone(),
                        body: rest[..n].to_vec(),
                    });
                }
            }
            // Neither: fall through and read until EOF.
        }

        if buf.len() > RESPONSE_MAX {
            return Err(VpnError::Protocol("HTTP response exceeded size guard".into()));
        }

        let n = match stream.read(&mut chunk).await {
            Ok(n) => n,
            // rustls surfaces a close without `close_notify` this way; whatever is
            // buffered is what the server sent.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => 0,
            Err(e) => return Err(e.into()),
        };
        if n == 0 {
            // EOF: the head must at least be complete, and whatever follows is
            // the body (the "read until close" case).
            let Some((end, status, headers)) = head else {
                return Err(VpnError::Protocol(
                    "connection closed before HTTP headers completed".into(),
                ));
            };
            return Ok(HttpResponse { status, headers, body: buf[end..].to_vec() });
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Issue one request over an existing stream and read the response.
pub async fn request_on<S>(
    stream: &mut S,
    method: &str,
    path: &str,
    host: &str,
    user_agent: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> Result<HttpResponse, VpnError>
where
    S: AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let req = build_request(method, path, host, user_agent, cookie, body);
    stream.write_all(req.as_bytes()).await?;
    stream.flush().await?;
    tokio::time::timeout(REQUEST_TIMEOUT, read_response(stream))
        .await
        .map_err(|_| {
            VpnError::Tls(format!("timed out reading FortiGate response for {method} {path}"))
        })?
}

/// Issue one request over a fresh short-lived TLS connection.
pub async fn request(
    host: &str,
    port: u16,
    trust: &CertTrust,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Option<&str>,
) -> Result<HttpResponse, VpnError> {
    let mut tls = connect_tls(host, port, trust).await?;
    request_on(&mut tls, method, path, host, USER_AGENT, cookie, body).await
}

/// A keep-alive HTTP session against one gateway.
///
/// The pre-tunnel flow is five requests (login page, logincheck, host-check
/// redirect, allocation warm-up, config XML). Opening a TLS connection per
/// request costs a full handshake each — measured at ~1.7 s against a real
/// gateway, so ~8 s of pure handshake before the tunnel even starts. FortiOS
/// keeps HTTP/1.1 connections alive, so this reuses one. If the server does
/// close (or the connection went stale between steps), the request is retried
/// once on a fresh connection.
pub struct HttpSession {
    host: String,
    port: u16,
    trust: CertTrust,
    stream: Option<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
}

impl HttpSession {
    pub fn new(host: &str, port: u16, trust: &CertTrust) -> Self {
        Self { host: host.to_string(), port, trust: trust.clone(), stream: None }
    }

    /// Issue a request, transparently (re)connecting as needed.
    pub async fn request(
        &mut self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        body: Option<&str>,
    ) -> Result<HttpResponse, VpnError> {
        for attempt in 0..2 {
            if self.stream.is_none() {
                self.stream = Some(connect_tls(&self.host, self.port, &self.trust).await?);
            }
            let stream = self.stream.as_mut().expect("just connected");
            let out =
                request_on(stream, method, path, &self.host, USER_AGENT, cookie, body).await;
            match out {
                Ok(r) => {
                    // Honor an explicit close so the next request reconnects.
                    if r.header("connection")
                        .is_some_and(|v| v.eq_ignore_ascii_case("close"))
                    {
                        self.stream = None;
                    }
                    return Ok(r);
                }
                Err(e) => {
                    // A reused connection the server had already closed fails on
                    // the first write/read; retry once on a fresh one.
                    self.stream = None;
                    if attempt == 1 {
                        return Err(e);
                    }
                    tracing::debug!(error = %e, path, "reconnecting FortiGate HTTP session");
                }
            }
        }
        unreachable!("loop returns on both attempts")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(raw: &str) -> HttpResponse {
        let mut cur = std::io::Cursor::new(raw.as_bytes().to_vec());
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(read_response(&mut cur))
            .unwrap()
    }

    #[test]
    fn reads_content_length_body_without_waiting_for_eof() {
        let r = resp("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloTRAILING-GARBAGE");
        assert_eq!(r.status, 200);
        assert_eq!(r.body_str(), "hello");
    }

    #[test]
    fn decodes_chunked_body() {
        // This is what real FortiOS sends for /remote/logincheck.
        let r = resp(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/plain\r\n\r\n\
             9\r\nret=1,red\r\n3\r\nir=\r\n0\r\n\r\n",
        );
        assert_eq!(r.status, 200);
        assert_eq!(r.body_str(), "ret=1,redir=");
    }

    #[test]
    fn falls_back_to_eof_when_undelimited() {
        let r = resp("HTTP/1.1 200 OK\r\nServer: x\r\n\r\nbody-to-eof");
        assert_eq!(r.body_str(), "body-to-eof");
    }

    #[test]
    fn parses_status_and_lowercases_header_names() {
        let r = resp("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nX-Odd-CASE: Yes\r\n\r\n");
        assert_eq!(r.status, 403);
        assert_eq!(r.header("x-odd-case"), Some("Yes"));
        assert_eq!(r.header("X-ODD-CASE"), Some("Yes"));
    }

    #[test]
    fn cookie_skips_cleared_value_and_keeps_usable_one() {
        // Exactly the shape FortiOS returns from /remote/logincheck when a
        // host-check is pending: SVPNCOOKIE emptied, SVPNTMPCOOKIE issued.
        let r = resp(
            "HTTP/1.1 200 OK\r\n\
             Set-Cookie:  SVPNCOOKIE=; path=/; expires=Sun, 11 Mar 1984 12:00:00 GMT; secure\r\n\
             Set-Cookie: SVPNTMPCOOKIE=TMP123; path=/remote/hostcheck_install; secure\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert_eq!(r.cookie("SVPNCOOKIE"), None);
        assert_eq!(r.cookie("SVPNTMPCOOKIE").as_deref(), Some("TMP123"));
    }

    #[test]
    fn cookie_name_match_is_not_a_substring_match() {
        // `SVPNCOOKIE` must not be found inside `SVPNTMPCOOKIE`.
        let r = resp(
            "HTTP/1.1 200 OK\r\nSet-Cookie: SVPNTMPCOOKIE=abc; path=/\r\nContent-Length: 0\r\n\r\n",
        );
        assert_eq!(r.cookie("SVPNCOOKIE"), None);
    }

    #[test]
    fn cookie_takes_last_usable_value() {
        let r = resp(
            "HTTP/1.1 200 OK\r\n\
             Set-Cookie: SVPNCOOKIE=old; path=/\r\n\
             Set-Cookie: SVPNCOOKIE=new; path=/\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert_eq!(r.cookie("SVPNCOOKIE").as_deref(), Some("new"));
    }

    #[test]
    fn bad_chunk_size_is_a_protocol_error() {
        let mut cur = std::io::Cursor::new(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nZZ\r\nx\r\n".to_vec(),
        );
        let out = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(read_response(&mut cur));
        assert!(matches!(out, Err(VpnError::Protocol(_))));
    }

    #[test]
    fn login_request_shape() {
        let r = build_request(
            "POST", "/remote/logincheck", "vpn.example.com", USER_AGENT, None,
            Some("username=a&credential=b&realm=&ajax=1"),
        );
        assert!(r.starts_with("POST /remote/logincheck HTTP/1.1\r\n"));
        assert!(r.contains("Host: vpn.example.com"));
        assert!(r.contains("Content-Length: 37"));
        assert!(!r.contains("SV1"), "login UA must not contain SV1 (HTTP 405)");
        assert!(r.ends_with("username=a&credential=b&realm=&ajax=1"));
    }
}
