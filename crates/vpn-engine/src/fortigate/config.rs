//! FortiGate tunnel configuration retrieval (FG-CFG-01).
//!
//! After auth, `GET /remote/fortisslvpn_xml` returns the tunnel parameters as
//! XML. openfortivpn also pings `/remote/index` and `/remote/fortisslvpn` first
//! to trigger the session allocation, so we do too (best-effort — a live gateway
//! answers 403 to `/remote/index` and 200 to `/remote/fortisslvpn`, and the
//! config fetch works either way).
//!
//! **Quoting.** Real FortiOS writes attributes with SINGLE quotes:
//! `<assigned-addr ipv4='10.0.3.1'/>`. An earlier version of this parser only
//! looked for `attr="`, so against every real gateway it found nothing and failed
//! with "missing assigned-addr ipv4". The scanner now accepts either quote style.
//!
//! A live reply looks like:
//! ```xml
//! <sslvpn-tunnel ver='2' dtls='1' patch='1'>
//!   <tunnel-method value='ppp' /><tunnel-method value='tun' />
//!   <auth-ses check-src-ip='1' tun-connect-without-reauth='0' />
//!   <ipv4><dns ip='172.29.0.25' /><wins ip='172.29.0.25' />
//!     <assigned-addr ipv4='10.0.3.1' />
//!     <split-tunnel-info><addr ip='10.10.0.0' mask='255.255.0.0' /></split-tunnel-info>
//!   </ipv4>
//!   <idle-timeout val='300' /><auth-timeout val='28800' />
//! </sslvpn-tunnel>
//! ```
//!
//! The XML is untrusted server input: parsing is a bounded, panic-free attribute
//! scan (no XML crate — deps are LOCKED).
#![allow(dead_code)]

use std::net::Ipv4Addr;

use super::http::{self, HttpSession};
use crate::error::VpnError;
use crate::tunnel::{CertTrust, SessionParams};

/// Fallback tunnel MTU. The XML carries no MTU; the real value comes from the
/// PPP MRU negotiated in `ppp.rs` (1354). This only applies if that is missing.
const DEFAULT_MTU: u16 = 1354;

/// Keepalive cadence when the gateway advertises no idle timeout.
const DEFAULT_KEEPALIVE_SECS: u32 = 30;

/// Parsed FortiGate tunnel configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FortiConfig {
    /// Assigned tunnel IPv4 address (`<assigned-addr ipv4=...>`), if the gateway
    /// pre-declares one. It is authoritative only as a cross-check: IPCP is what
    /// actually assigns the address.
    pub address: Option<Ipv4Addr>,
    /// Netmask derived from `prefix-len`, when present.
    pub netmask: Option<Ipv4Addr>,
    /// DNS resolvers (`<dns ip=...>`), zero or more.
    pub dns: Vec<Ipv4Addr>,
    /// DNS search domain (`<dns domain=...>`), if present.
    pub dns_suffix: Option<String>,
    /// Split-tunnel INCLUDE routes. Empty means the server pushed no list — a
    /// full tunnel; the caller decides how to route that.
    pub routes: Vec<(Ipv4Addr, u8)>,
    /// Networks the gateway explicitly EXCLUDES (`<split-tunnel-info negate='1'>`).
    /// Folding these into `routes` would hijack traffic meant to stay local.
    pub exclude_routes: Vec<(Ipv4Addr, u8)>,
    /// Wire protocol version from `<sslvpn-tunnel ver=...>`.
    pub version: Option<String>,
    /// Advertised tunnel methods (`ppp`, `tun`). Informational: `GET
    /// /remote/sslvpn-tunnel` yields PPP regardless.
    pub tunnel_methods: Vec<String>,
    /// `<idle-timeout val=...>` in seconds — the gateway drops a silent tunnel
    /// after this, so it sets the keepalive cadence.
    pub idle_timeout: Option<u32>,
    /// `<auth-timeout val=...>` in seconds — hard session lifetime.
    pub auth_timeout: Option<u32>,
}

impl FortiConfig {
    /// Map into the protocol-agnostic [`SessionParams`]. `ppp` carries whatever
    /// LCP/IPCP negotiated, which wins over the XML where the two overlap.
    pub fn to_session_params(&self, ppp: Option<&super::ppp::PppSession>) -> SessionParams {
        let address = ppp
            .map(|p| p.address)
            .or(self.address)
            .unwrap_or(Ipv4Addr::UNSPECIFIED);

        // Prefer IPCP's resolvers, then the XML's, deduped and order-preserving.
        let mut dns: Vec<Ipv4Addr> = Vec::new();
        for a in ppp.map(|p| p.dns.as_slice()).unwrap_or(&[]).iter().chain(self.dns.iter()) {
            if !dns.contains(a) {
                dns.push(*a);
            }
        }

        SessionParams {
            address,
            netmask: self.netmask,
            dns,
            mtu: ppp.map(|p| p.mtu).unwrap_or(DEFAULT_MTU),
            keepalive: Some(self.keepalive_secs()),
            dpd: None,
            disconnected_timeout: self.idle_timeout,
        }
    }

    /// Keepalive cadence: comfortably inside the gateway's idle timeout (a third
    /// of it, so two probes can be lost before it expires), clamped to 5..=60 s.
    pub fn keepalive_secs(&self) -> u32 {
        match self.idle_timeout {
            Some(t) if t > 0 => (t / 3).clamp(5, 60),
            _ => DEFAULT_KEEPALIVE_SECS,
        }
    }
}

/// Convert a dotted netmask (`255.255.255.0`) to a prefix length (`24`).
fn mask_to_prefix(mask: Ipv4Addr) -> u8 {
    u32::from(mask).count_ones() as u8
}

/// Convert a prefix length to a dotted netmask.
fn prefix_to_mask(prefix: u8) -> Ipv4Addr {
    let p = prefix.min(32);
    let bits: u32 = if p == 0 { 0 } else { u32::MAX << (32 - p) };
    Ipv4Addr::from(bits)
}

/// Extract the value of `attr='...'` or `attr="..."` from an element fragment.
///
/// The attribute name must be matched WHOLE: a naive `find("ip=")` would also
/// hit the `ip=` inside `prefix-len=`-style names, and `<addr ipv6=...>` must not
/// satisfy a lookup for `ip`.
fn attr(fragment: &str, name: &str) -> Option<String> {
    let bytes = fragment.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = fragment[from..].find(name) {
        let start = from + rel;
        let after = start + name.len();
        from = after;
        // Left boundary: the previous char must not extend the name.
        if start > 0 {
            let p = bytes[start - 1];
            if p.is_ascii_alphanumeric() || p == b'-' || p == b'_' {
                continue;
            }
        }
        // Right boundary: optional spaces, then '=', then a quote.
        let rest = &fragment[after..];
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let quote = match rest.as_bytes().first() {
            Some(b'\'') => '\'',
            Some(b'"') => '"',
            _ => continue,
        };
        let rest = &rest[1..];
        let end = rest.find(quote)?;
        return Some(rest[..end].to_string());
    }
    None
}

/// Parse the `fortisslvpn_xml` body (FG-CFG-02). Scans element fragments rather
/// than building a DOM — the schema is flat and the input is untrusted, so a
/// bounded string scan is simpler and panic-free.
pub fn parse_config_xml(body: &str) -> Result<FortiConfig, VpnError> {
    let mut cfg = FortiConfig::default();
    // `<split-tunnel-info>` scopes the `<addr>` entries that follow it, and its
    // `negate` attribute flips them from includes to excludes.
    let mut in_split = false;
    let mut split_negated = false;

    for frag in body.split('<') {
        let frag = frag.trim_start();
        let name_end = frag
            .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .unwrap_or(frag.len());
        let (name, rest) = frag.split_at(name_end);

        match name {
            "sslvpn-tunnel" => cfg.version = attr(rest, "ver"),
            "tunnel-method" => {
                if let Some(v) = attr(rest, "value") {
                    cfg.tunnel_methods.push(v);
                }
            }
            "idle-timeout" => cfg.idle_timeout = attr(rest, "val").and_then(|v| v.parse().ok()),
            "auth-timeout" => cfg.auth_timeout = attr(rest, "val").and_then(|v| v.parse().ok()),
            "assigned-addr" => {
                if let Some(ip) = attr(rest, "ipv4").and_then(|v| v.parse().ok()) {
                    cfg.address = Some(ip);
                }
                if let Some(p) = attr(rest, "prefix-len").and_then(|v| v.parse::<u8>().ok()) {
                    cfg.netmask = Some(prefix_to_mask(p));
                }
            }
            // `<wins ...>` deliberately not matched: those are NetBIOS servers.
            "dns" => {
                if let Some(a) = attr(rest, "ip").and_then(|v| v.parse::<Ipv4Addr>().ok()) {
                    if !cfg.dns.contains(&a) {
                        cfg.dns.push(a);
                    }
                }
                if let Some(d) = attr(rest, "domain") {
                    if !d.is_empty() {
                        cfg.dns_suffix = Some(d);
                    }
                }
            }
            "split-tunnel-info" => {
                in_split = true;
                split_negated = matches!(attr(rest, "negate").as_deref(), Some("1") | Some("true"));
            }
            "/split-tunnel-info" => {
                in_split = false;
                split_negated = false;
            }
            "addr" if in_split => {
                // IPv6 entries use `ipv6`/`prefix-len` and are skipped (v4-only).
                let Some(ip) = attr(rest, "ip").and_then(|v| v.parse::<Ipv4Addr>().ok()) else {
                    continue;
                };
                let prefix = match attr(rest, "mask").and_then(|v| v.parse::<Ipv4Addr>().ok()) {
                    Some(m) => mask_to_prefix(m),
                    None => match attr(rest, "prefix-len").and_then(|v| v.parse::<u8>().ok()) {
                        Some(p) => p,
                        None => continue,
                    },
                };
                if split_negated {
                    cfg.exclude_routes.push((ip, prefix));
                } else {
                    cfg.routes.push((ip, prefix));
                }
            }
            _ => {}
        }
    }
    Ok(cfg)
}

/// Fetch and parse the tunnel configuration over an existing HTTP session.
pub async fn fetch_config_on(
    session: &mut HttpSession,
    cookie: &str,
) -> Result<FortiConfig, VpnError> {
    let ck = format!("SVPNCOOKIE={cookie}");

    // Allocation warm-up (openfortivpn ordering). A live gateway answers 403 to
    // /remote/index; that is not fatal, so neither step blocks the config fetch.
    for path in ["/remote/index", "/remote/fortisslvpn"] {
        match session.request("GET", path, Some(&ck), None).await {
            Ok(r) => tracing::debug!(path, status = r.status, "FortiGate allocation warm-up"),
            Err(e) => tracing::debug!(path, error = %e, "FortiGate allocation warm-up failed"),
        }
    }

    let resp = session
        .request("GET", "/remote/fortisslvpn_xml", Some(&ck), None)
        .await?;
    if resp.status >= 400 {
        return Err(VpnError::Protocol(format!(
            "FortiGate config fetch failed (HTTP {})",
            resp.status
        )));
    }
    let cfg = parse_config_xml(&resp.body_str())?;
    tracing::info!(
        version = cfg.version.as_deref().unwrap_or("?"),
        methods = ?cfg.tunnel_methods,
        address = ?cfg.address,
        dns_count = cfg.dns.len(),
        route_count = cfg.routes.len(),
        exclude_count = cfg.exclude_routes.len(),
        idle_timeout = ?cfg.idle_timeout,
        "FortiGate tunnel config retrieved"
    );
    Ok(cfg)
}

/// Convenience wrapper that owns its own HTTP session.
pub async fn fetch_config(
    host: &str,
    port: u16,
    trust: &CertTrust,
    cookie: &str,
) -> Result<FortiConfig, VpnError> {
    let mut session = http::HttpSession::new(host, port, trust);
    fetch_config_on(&mut session, cookie).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim body from a live FortiOS 7.x gateway (`vpn2`, portal `Portal_HM`).
    const LIVE_XML: &str = "<?xml version='1.0' encoding='utf-8'?><sslvpn-tunnel ver='2' dtls='1' patch='1'><dtls-config heartbeat-interval='10' heartbeat-fail-count='10' heartbeat-idle-timeout='10' client-hello-timeout='10' /><tunnel-method value='ppp' /><tunnel-method value='tun' /><auth-ses check-src-ip='1' tun-connect-without-reauth='0' tun-user-ses-timeout='30' /><client-config save-password='off' keep-alive='off' auto-connect='off' /><ipv4><dns ip='172.29.0.25' /><dns ip='172.29.0.23' /><wins ip='172.29.0.25' /><wins ip='172.29.0.23' /><assigned-addr ipv4='10.0.3.1' /><split-tunnel-info><addr ip='10.10.0.0' mask='255.255.0.0' /><addr ip='172.29.0.111' mask='255.255.255.255' /><addr ip='172.29.0.23' mask='255.255.255.255' /><addr ip='172.29.0.25' mask='255.255.255.255' /><addr ip='172.29.0.228' mask='255.255.255.255' /><addr ip='10.10.28.0' mask='255.255.252.0' /><addr ip='172.27.68.0' mask='255.255.254.0' /><addr ip='172.29.0.29' mask='255.255.255.255' /><addr ip='172.27.68.232' mask='255.255.255.255' /><addr ip='172.29.0.255' mask='255.255.255.255' /></split-tunnel-info></ipv4><idle-timeout val='300' /><auth-timeout val='28800' /></sslvpn-tunnel>";

    #[test]
    fn parses_the_live_gateway_reply() {
        let c = parse_config_xml(LIVE_XML).unwrap();
        assert_eq!(c.version.as_deref(), Some("2"));
        assert_eq!(c.tunnel_methods, vec!["ppp", "tun"]);
        assert_eq!(c.address, Some("10.0.3.1".parse().unwrap()));
        assert_eq!(
            c.dns,
            vec![
                "172.29.0.25".parse::<Ipv4Addr>().unwrap(),
                "172.29.0.23".parse::<Ipv4Addr>().unwrap()
            ],
            "`wins` entries must not be picked up as DNS"
        );
        assert_eq!(c.routes.len(), 10);
        assert_eq!(c.routes[0], ("10.10.0.0".parse().unwrap(), 16));
        assert_eq!(c.routes[5], ("10.10.28.0".parse().unwrap(), 22));
        assert_eq!(c.routes[6], ("172.27.68.0".parse().unwrap(), 23));
        assert!(c.exclude_routes.is_empty());
        assert_eq!(c.idle_timeout, Some(300));
        assert_eq!(c.auth_timeout, Some(28800));
    }

    #[test]
    fn single_and_double_quotes_both_parse() {
        // The double-quoted form is what the old tests used; both must work.
        let dq = r#"<sslvpn-tunnel ver="2"><ipv4><assigned-addr ipv4="10.1.2.3" /></ipv4></sslvpn-tunnel>"#;
        assert_eq!(parse_config_xml(dq).unwrap().address, Some("10.1.2.3".parse().unwrap()));
        let sq = "<sslvpn-tunnel ver='2'><ipv4><assigned-addr ipv4='10.1.2.3' /></ipv4></sslvpn-tunnel>";
        assert_eq!(parse_config_xml(sq).unwrap().address, Some("10.1.2.3".parse().unwrap()));
    }

    #[test]
    fn negated_split_tunnel_becomes_excludes() {
        let xml = "<sslvpn-tunnel><ipv4><assigned-addr ipv4='10.1.2.3'/>\
                   <split-tunnel-info negate='1'><addr ip='192.168.0.0' mask='255.255.0.0'/>\
                   </split-tunnel-info></ipv4></sslvpn-tunnel>";
        let c = parse_config_xml(xml).unwrap();
        assert!(c.routes.is_empty(), "a negated list must not become include routes");
        assert_eq!(c.exclude_routes, vec![("192.168.0.0".parse().unwrap(), 16)]);
    }

    #[test]
    fn addr_outside_split_tunnel_info_is_ignored() {
        let xml = "<sslvpn-tunnel><ipv4><addr ip='1.2.3.4' mask='255.255.255.0'/>\
                   <assigned-addr ipv4='10.1.2.3'/></ipv4></sslvpn-tunnel>";
        assert!(parse_config_xml(xml).unwrap().routes.is_empty());
    }

    #[test]
    fn ipv6_split_entries_are_skipped() {
        let xml = "<sslvpn-tunnel><ipv6><split-tunnel-info>\
                   <addr ipv6='2001:db8::' prefix-len='32'/></split-tunnel-info></ipv6>\
                   <ipv4><assigned-addr ipv4='10.1.2.3'/></ipv4></sslvpn-tunnel>";
        assert!(parse_config_xml(xml).unwrap().routes.is_empty());
    }

    #[test]
    fn prefix_len_becomes_a_netmask() {
        let xml = "<sslvpn-tunnel><ipv4><assigned-addr ipv4='10.1.2.3' prefix-len='24'/></ipv4></sslvpn-tunnel>";
        let c = parse_config_xml(xml).unwrap();
        assert_eq!(c.netmask, Some("255.255.255.0".parse().unwrap()));
    }

    #[test]
    fn attr_requires_a_whole_name_match() {
        assert_eq!(attr(" ipv6='::1' ", "ip"), None, "`ip` must not match `ipv6`");
        assert_eq!(attr(" prefix-len='24' ", "len"), None, "`len` must not match `prefix-len`");
        assert_eq!(attr(" ip='1.2.3.4' mask='x' ", "ip").as_deref(), Some("1.2.3.4"));
        assert_eq!(attr(" ip = '1.2.3.4' ", "ip").as_deref(), Some("1.2.3.4"));
    }

    #[test]
    fn masks_and_prefixes_round_trip() {
        for p in [0u8, 8, 16, 22, 23, 24, 32] {
            assert_eq!(mask_to_prefix(prefix_to_mask(p)), p);
        }
        assert_eq!(mask_to_prefix("255.255.252.0".parse().unwrap()), 22);
    }

    #[test]
    fn full_tunnel_has_no_routes() {
        let xml = "<sslvpn-tunnel><ipv4><assigned-addr ipv4='10.1.2.3'/><dns ip='1.1.1.1'/></ipv4></sslvpn-tunnel>";
        let c = parse_config_xml(xml).unwrap();
        assert!(c.routes.is_empty(), "no split-tunnel-info => full tunnel");
        assert_eq!(c.dns_suffix, None);
    }

    #[test]
    fn keepalive_stays_inside_the_idle_timeout() {
        let mut c = FortiConfig { idle_timeout: Some(300), ..Default::default() };
        assert_eq!(c.keepalive_secs(), 60, "300/3 clamped to the 60s ceiling");
        c.idle_timeout = Some(30);
        assert_eq!(c.keepalive_secs(), 10);
        c.idle_timeout = Some(6);
        assert_eq!(c.keepalive_secs(), 5, "clamped to the 5s floor");
        c.idle_timeout = None;
        assert_eq!(c.keepalive_secs(), DEFAULT_KEEPALIVE_SECS);
    }

    #[test]
    fn session_params_prefer_ipcp_over_xml() {
        let c = parse_config_xml(LIVE_XML).unwrap();
        let ppp = super::super::ppp::PppSession {
            address: "10.0.9.9".parse().unwrap(),
            peer: None,
            dns: vec!["8.8.8.8".parse().unwrap(), "172.29.0.25".parse().unwrap()],
            mtu: 1354,
            magic: 1,
        };
        let p = c.to_session_params(Some(&ppp));
        assert_eq!(p.address, "10.0.9.9".parse::<Ipv4Addr>().unwrap());
        assert_eq!(p.mtu, 1354);
        // IPCP resolvers first, XML-only ones appended, no duplicates.
        assert_eq!(
            p.dns,
            vec![
                "8.8.8.8".parse::<Ipv4Addr>().unwrap(),
                "172.29.0.25".parse::<Ipv4Addr>().unwrap(),
                "172.29.0.23".parse::<Ipv4Addr>().unwrap(),
            ]
        );
        assert_eq!(p.keepalive, Some(60));
    }

    #[test]
    fn session_params_fall_back_to_xml_without_ppp() {
        let c = parse_config_xml(LIVE_XML).unwrap();
        let p = c.to_session_params(None);
        assert_eq!(p.address, "10.0.3.1".parse::<Ipv4Addr>().unwrap());
        assert_eq!(p.mtu, DEFAULT_MTU);
    }
}
