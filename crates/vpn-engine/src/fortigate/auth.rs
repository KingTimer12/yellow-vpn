//! FortiGate SSL VPN username/password authentication (FG-AUTH-01).
//!
//! POSTs an `x-www-form-urlencoded` login to `/remote/logincheck` and extracts
//! the `SVPNCOOKIE` session cookie, which is the credential for the config fetch
//! and the tunnel upgrade.
//!
//! **Host check.** A plain "look for `Set-Cookie: SVPNCOOKIE`" implementation
//! fails against any portal with host checking enabled — and that is the common
//! case. Such a gateway answers the login with HTTP 200,
//! `ret=1,redir=/remote/hostcheck_install?...`, a *cleared* `SVPNCOOKIE` (empty
//! value, expiry in 1984) and a fresh `SVPNTMPCOOKIE` scoped to
//! `/remote/hostcheck_install`. The real `SVPNCOOKIE` is only issued when that
//! redirect is followed carrying the temporary cookie. Verified against a live
//! FortiOS 7.x portal, which reports `auth_type=16`.
//!
//! Security: credentials travel only over the established TLS channel and are
//! NEVER logged; form values are percent-encoded so a crafted credential cannot
//! alter the request. `SVPNCOOKIE`/`SVPNTMPCOOKIE` are credential-equivalent and
//! never logged.
//!
//! Scope: the username/password path (with host check). An OTP/2FA challenge is
//! detected and surfaced as a precise `AuthFailed` rather than a generic one —
//! completing that round needs an interactive channel for the code, which the
//! IPC surface does not carry yet.
#![allow(dead_code)]

use super::http::{self, HttpResponse, HttpSession};
use crate::error::VpnError;
use crate::tunnel::CertTrust;

/// Percent-encode a form value: everything outside the unreserved set
/// (`A-Z a-z 0-9 - _ . ~`, RFC 3986) is escaped as `%XX`.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the `/remote/logincheck` form body. `realm` is usually empty; portals
/// configured with realms reject a login that omits theirs.
pub fn build_login_body(username: &str, password: &str, realm: &str) -> String {
    format!(
        "username={}&credential={}&realm={}&ajax=1",
        url_encode(username),
        url_encode(password),
        url_encode(realm)
    )
}

/// A field parsed out of the comma-separated `logincheck` reply body
/// (`ret=1,redir=/remote/...,tokeninfo=...`).
pub fn body_field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    for part in body.trim().split(',') {
        if let Some(v) = part.trim().strip_prefix(needle.as_str()) {
            return Some(v);
        }
    }
    None
}

/// What the gateway wants after the credentials were accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    /// The session cookie was issued directly.
    Cookie(String),
    /// A host check stands between us and the cookie: follow `redir` while
    /// sending `SVPNTMPCOOKIE=<tmp_cookie>`.
    HostCheck { redir: String, tmp_cookie: Option<String> },
    /// An OTP / second factor is required.
    TwoFactor(String),
}

/// Classify the login response (FG-AUTH-02). All input here is untrusted server
/// data. Checks run in order:
/// 1. A `4xx`/`5xx` status → PERMANENT [`VpnError::AuthFailed`].
/// 2. A usable `SVPNCOOKIE` → done.
/// 3. `ret=1` plus a `redir` → host-check round.
/// 4. 2FA markers (`tokeninfo`/`reqid`/`polid`/`magic`) → `TwoFactor`.
/// 5. Anything else → `AuthFailed`.
pub fn classify_login(resp: &HttpResponse) -> Result<LoginOutcome, VpnError> {
    if resp.status >= 400 {
        return Err(VpnError::AuthFailed(format!(
            "server rejected credentials (HTTP {})",
            resp.status
        )));
    }
    if let Some(c) = resp.cookie("SVPNCOOKIE") {
        return Ok(LoginOutcome::Cookie(c));
    }

    let body = resp.body_str();
    let ret = body_field(&body, "ret").unwrap_or("");

    if ret == "1" {
        if let Some(redir) = body_field(&body, "redir") {
            if !redir.is_empty() {
                return Ok(LoginOutcome::HostCheck {
                    redir: redir.to_string(),
                    tmp_cookie: resp.cookie("SVPNTMPCOOKIE"),
                });
            }
        }
    }

    // A second factor: FortiGate replays the login context and expects
    // `code=&code2=&magic=` on a follow-up POST.
    if ["tokeninfo", "reqid", "polid", "magic"]
        .iter()
        .any(|k| body_field(&body, k).is_some())
    {
        return Ok(LoginOutcome::TwoFactor(
            body_field(&body, "tokeninfo").unwrap_or_default().to_string(),
        ));
    }

    Err(VpnError::AuthFailed(if ret.is_empty() {
        "no SVPNCOOKIE in login response and no recognizable status field".into()
    } else {
        format!("gateway rejected the login (ret={ret})")
    }))
}

/// Authenticate and return the `SVPNCOOKIE`, reusing `session`'s connection so
/// the caller can go straight on to the config fetch. Credentials and cookies
/// are NEVER logged.
pub async fn authenticate_on(
    session: &mut HttpSession,
    username: &str,
    password: &str,
    realm: &str,
) -> Result<String, VpnError> {
    // Prime the portal session. openfortivpn does this first and some portals
    // will not hand out a host-check context without it; a failure is not fatal.
    if let Err(e) = session.request("GET", "/remote/login?lang=en", None, None).await {
        tracing::debug!(error = %e, "portal login page fetch failed (continuing)");
    }

    let body = build_login_body(username, password, realm);
    let resp = session
        .request("POST", "/remote/logincheck", None, Some(&body))
        .await?;

    let cookie = match classify_login(&resp)? {
        LoginOutcome::Cookie(c) => c,
        LoginOutcome::HostCheck { redir, tmp_cookie } => {
            tracing::info!("FortiGate portal requires a host check — following the redirect");
            let hdr = tmp_cookie.map(|t| format!("SVPNTMPCOOKIE={t}"));
            let resp2 = session.request("GET", &redir, hdr.as_deref(), None).await?;
            if resp2.status >= 400 {
                return Err(VpnError::AuthFailed(format!(
                    "host check failed (HTTP {})",
                    resp2.status
                )));
            }
            resp2.cookie("SVPNCOOKIE").ok_or_else(|| {
                VpnError::AuthFailed(
                    "host check completed but the gateway issued no SVPNCOOKIE".into(),
                )
            })?
        }
        LoginOutcome::TwoFactor(info) => {
            return Err(VpnError::AuthFailed(format!(
                "gateway requires a second factor (OTP){}; interactive OTP is not supported yet",
                if info.is_empty() { String::new() } else { format!(" [{info}]") }
            )));
        }
    };

    tracing::info!(host = %"<gateway>", "FortiGate authentication succeeded");
    Ok(cookie)
}

/// Convenience wrapper that owns its own HTTP session.
pub async fn authenticate_fortigate(
    host: &str,
    port: u16,
    trust: &CertTrust,
    username: &str,
    password: &str,
) -> Result<String, VpnError> {
    let mut session = http::HttpSession::new(host, port, trust);
    authenticate_on(&mut session, username, password, "").await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(raw: &str) -> HttpResponse {
        let mut cur = std::io::Cursor::new(raw.as_bytes().to_vec());
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(http::read_response(&mut cur))
            .unwrap()
    }

    #[test]
    fn login_body_has_form_fields() {
        assert_eq!(
            build_login_body("alice", "s3cret", ""),
            "username=alice&credential=s3cret&realm=&ajax=1"
        );
    }

    #[test]
    fn credentials_are_percent_encoded() {
        let b = build_login_body("a b", "p&w=x", "corp realm");
        assert_eq!(b, "username=a%20b&credential=p%26w%3Dx&realm=corp%20realm&ajax=1");
        assert!(!b.contains("p&w=x"), "an unescaped & would forge extra form fields");
    }

    #[test]
    fn password_with_at_sign_is_encoded() {
        // '@' is outside the RFC 3986 unreserved set.
        assert!(build_login_body("u", "wa@1921", "").contains("credential=wa%401921"));
    }

    #[test]
    fn body_field_picks_the_right_key() {
        let b = "ret=1,redir=/remote/hostcheck_install?a=1&b=2,realm=";
        assert_eq!(body_field(b, "ret"), Some("1"));
        assert_eq!(body_field(b, "redir"), Some("/remote/hostcheck_install?a=1&b=2"));
        assert_eq!(body_field(b, "nope"), None);
    }

    #[test]
    fn direct_cookie_is_success() {
        let r = resp(
            "HTTP/1.1 200 OK\r\nSet-Cookie: SVPNCOOKIE=abc123; path=/\r\nContent-Length: 0\r\n\r\n",
        );
        assert_eq!(classify_login(&r).unwrap(), LoginOutcome::Cookie("abc123".into()));
    }

    #[test]
    fn hostcheck_round_is_detected() {
        // Byte-for-byte the shape a live FortiOS 7.x portal returns.
        let r = resp(
            "HTTP/1.1 200 OK\r\n\
             Set-Cookie:  SVPNCOOKIE=; path=/; expires=Sun, 11 Mar 1984 12:00:00 GMT; secure\r\n\
             Set-Cookie: SVPNTMPCOOKIE=TMPVAL; path=/remote/hostcheck_install; secure\r\n\
             Transfer-Encoding: chunked\r\nContent-Type: text/plain\r\n\r\n\
             65\r\nret=1,redir=/remote/hostcheck_install?auth_type=16&user=7769&&grpname=&portal=506F&rip=1.2.3.4&realm=\r\n0\r\n\r\n",
        );
        match classify_login(&r).unwrap() {
            LoginOutcome::HostCheck { redir, tmp_cookie } => {
                assert!(redir.starts_with("/remote/hostcheck_install?auth_type=16"));
                assert_eq!(tmp_cookie.as_deref(), Some("TMPVAL"));
            }
            other => panic!("expected HostCheck, got {other:?}"),
        }
    }

    #[test]
    fn http_403_is_auth_failed() {
        let r = resp("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        assert!(matches!(classify_login(&r), Err(VpnError::AuthFailed(_))));
    }

    #[test]
    fn two_factor_challenge_is_reported_precisely() {
        let r = resp(
            "HTTP/1.1 200 OK\r\nContent-Length: 44\r\n\r\n\
             ret=2,reqid=17,polid=3,grp=,portal=x,magic=99",
        );
        assert!(matches!(classify_login(&r), Ok(LoginOutcome::TwoFactor(_))));
    }

    #[test]
    fn plain_rejection_is_auth_failed() {
        let r = resp("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nret=0");
        let e = classify_login(&r).unwrap_err();
        assert!(matches!(e, VpnError::AuthFailed(_)));
        assert!(e.to_string().contains("ret=0"));
    }

    #[test]
    fn cleared_cookie_is_not_mistaken_for_success() {
        let r = resp(
            "HTTP/1.1 200 OK\r\n\
             Set-Cookie: SVPNCOOKIE=0; path=/\r\nContent-Length: 5\r\n\r\nret=0",
        );
        assert!(matches!(classify_login(&r), Err(VpnError::AuthFailed(_))));
    }
}
