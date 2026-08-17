//! FortiGate SSL VPN protocol layer (FG).
//!
//! Flow: HTTPS auth (with the host-check round) -> `SVPNCOOKIE` -> config XML ->
//! `GET /remote/sslvpn-tunnel` upgrade -> `0x5050`-framed **PPP** over TLS ->
//! LCP + IPCP -> raw IPv4 in protocol `0x0021`.
//!
//! This targets the **v1** wire protocol, which is what real gateways speak. The
//! `v2` (non-PPP, raw-IP) protocol is advertised by FortiOS >= 5.6.6 but no
//! public client implements it and nothing documents how to select it — a live
//! FortiOS 7.x gateway reporting `ver='2'` and offering
//! `<tunnel-method value='tun'/>` still negotiates PPP on this endpoint.
#![allow(dead_code)]

pub mod auth;
pub mod config;
pub mod framing;
pub mod http;
pub mod ppp;
pub mod session;
