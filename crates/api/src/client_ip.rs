//! Who is on the other end of a request, for rate limiting and lockout.
//!
//! The socket peer is the client unless it is a configured trusted proxy
//! (`trusted_proxies`). Only then are forwarding headers read, and only
//! the part of them a trusted hop wrote: the rightmost `X-Forwarded-For`
//! entry that is not itself a trusted proxy. The leftmost entry is
//! whatever the caller typed, so believing it hands every request a fresh
//! rate-limit bucket and a fresh lockout counter. `CF-Connecting-IP` is
//! read only when `trust_cf_connecting_ip` says a Cloudflare hop sets it.
//!
//! ## What a client is counted as
//!
//! Every rate-limit bucket and the sign-in lockout count a client by
//! [`rate_key`]: an IPv4 address as it is, an IPv6 address by its /64. A
//! single IPv6 subscriber is normally given a whole /64 and can use any
//! address in it, so counting full addresses would hand one client 2^64
//! fresh buckets. An IPv4-mapped IPv6 address is the IPv4 address. The
//! exact address ([`resolve`]) is what logging and audit should record.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use vyasa_common::config::ProxyNet;

use crate::state::AppState;

/// The client address for `peer` and `headers` under the given trust.
/// `None` when there is no peer address at all (in-process tests).
#[must_use]
pub fn resolve(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    proxies: &[ProxyNet],
    trust_cf: bool,
) -> Option<IpAddr> {
    let peer = peer?.to_canonical();
    let trusted = |ip: IpAddr| proxies.iter().any(|p| p.contains(ip));
    if !trusted(peer) {
        return Some(peer);
    }
    if trust_cf {
        if let Some(ip) = headers
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_hop)
        {
            return Some(ip);
        }
    }
    // Every X-Forwarded-For line, in order, read from the right: each hop
    // appends the address it saw, so walking back through trusted hops
    // stops at the first address nobody we trust vouches past.
    let hops: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .collect();
    let mut client = peer;
    for hop in hops.iter().rev() {
        let Some(ip) = parse_hop(hop) else {
            // Garbage from here leftwards; the last hop we trusted is all
            // that can be said.
            break;
        };
        client = ip;
        if !trusted(ip) {
            break;
        }
    }
    Some(client)
}

fn parse_hop(raw: &str) -> Option<IpAddr> {
    let raw = raw.trim();
    raw.parse::<IpAddr>()
        .ok()
        .or_else(|| raw.parse::<SocketAddr>().ok().map(|s| s.ip()))
        .map(|ip| ip.to_canonical())
}

/// What a client is counted as by every rate limit and the lockout: an
/// IPv4 address as it is (an IPv4-mapped IPv6 address included), an IPv6
/// address as its /64 (`2001:db8:1:2::/64`). See the module documentation.
#[must_use]
pub fn rate_key(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            let net = Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0);
            format!("{net}/64")
        }
    }
}

/// The client's [`rate_key`] under this server's configuration;
/// `"unknown"` without a peer address.
#[must_use]
pub fn for_request(state: &AppState, peer: Option<IpAddr>, headers: &HeaderMap) -> String {
    resolve(
        peer,
        headers,
        &state.config.trusted_proxies,
        state.config.trust_cf_connecting_ip,
    )
    .map_or_else(|| String::from("unknown"), rate_key)
}

/// Extractor: the client's [`rate_key`], for rate limits and lockout
/// accounting (an IPv6 client is its /64). Not the exact address: for
/// logging or audit use [`resolve`].
pub struct ClientIp(pub String);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip());
        Ok(Self(for_request(state, peer, &parts.headers)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, v.parse().unwrap());
        }
        h
    }

    fn loopback() -> Vec<ProxyNet> {
        vec!["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()]
    }

    #[test]
    fn without_trusted_proxies_the_peer_is_the_client() {
        let h = headers(&[
            ("x-forwarded-for", "203.0.113.9"),
            ("cf-connecting-ip", "203.0.113.9"),
        ]);
        // Loopback is not special by default: a local caller cannot mint
        // addresses either.
        assert_eq!(
            resolve(Some(ip("127.0.0.1")), &h, &[], true),
            Some(ip("127.0.0.1"))
        );
        assert_eq!(
            resolve(Some(ip("198.51.100.7")), &h, &loopback(), true),
            Some(ip("198.51.100.7"))
        );
        assert_eq!(resolve(None, &h, &loopback(), true), None);
    }

    #[test]
    fn from_a_trusted_proxy_the_rightmost_untrusted_hop_is_the_client() {
        // The caller wrote 1.1.1.1 itself; the proxy appended what it saw.
        let h = headers(&[("x-forwarded-for", "1.1.1.1, 203.0.113.9")]);
        assert_eq!(
            resolve(Some(ip("127.0.0.1")), &h, &loopback(), false),
            Some(ip("203.0.113.9"))
        );
        // Chained trusted proxies are walked past.
        let proxies: Vec<ProxyNet> =
            vec!["127.0.0.1".parse().unwrap(), "10.0.0.0/8".parse().unwrap()];
        let h = headers(&[
            ("x-forwarded-for", "1.1.1.1, 203.0.113.9"),
            ("x-forwarded-for", "10.1.2.3"),
        ]);
        assert_eq!(
            resolve(Some(ip("127.0.0.1")), &h, &proxies, false),
            Some(ip("203.0.113.9"))
        );
        // No header: the proxy itself.
        assert_eq!(
            resolve(Some(ip("127.0.0.1")), &HeaderMap::new(), &loopback(), false),
            Some(ip("127.0.0.1"))
        );
        // Garbage stops the walk at the last trusted hop.
        let h = headers(&[("x-forwarded-for", "203.0.113.9, nonsense")]);
        assert_eq!(
            resolve(Some(ip("127.0.0.1")), &h, &loopback(), false),
            Some(ip("127.0.0.1"))
        );
    }

    #[test]
    fn an_ipv6_client_is_counted_by_its_slash_64() {
        // One subscriber is usually handed a whole /64 and can rotate
        // through it at will: every address in it is one client.
        assert_eq!(
            rate_key(ip("2001:db8:1:2:aaaa:bbbb:cccc:dddd")),
            "2001:db8:1:2::/64"
        );
        assert_eq!(
            rate_key(ip("2001:db8:1:2::1")),
            rate_key(ip("2001:db8:1:2:ffff:ffff:ffff:ffff"))
        );
        assert_ne!(
            rate_key(ip("2001:db8:1:2::1")),
            rate_key(ip("2001:db8:1:3::1"))
        );
        assert_eq!(rate_key(ip("::1")), "::/64");
        // IPv4 is as it is, and an IPv4-mapped IPv6 address is IPv4.
        assert_eq!(rate_key(ip("203.0.113.9")), "203.0.113.9");
        assert_eq!(rate_key(ip("::ffff:203.0.113.9")), "203.0.113.9");
        assert_ne!(rate_key(ip("203.0.113.9")), rate_key(ip("203.0.113.10")));
    }

    #[test]
    fn the_exact_address_is_kept_for_logging() {
        let h = headers(&[("x-forwarded-for", "2001:db8:1:2::abcd")]);
        assert_eq!(
            resolve(Some(ip("127.0.0.1")), &h, &loopback(), false),
            Some(ip("2001:db8:1:2::abcd"))
        );
    }

    #[test]
    fn cf_connecting_ip_counts_only_when_configured() {
        let h = headers(&[
            ("cf-connecting-ip", "203.0.113.9"),
            ("x-forwarded-for", "198.51.100.1"),
        ]);
        assert_eq!(
            resolve(Some(ip("127.0.0.1")), &h, &loopback(), true),
            Some(ip("203.0.113.9"))
        );
        assert_eq!(
            resolve(Some(ip("127.0.0.1")), &h, &loopback(), false),
            Some(ip("198.51.100.1"))
        );
        // Mapped loopback peers are loopback.
        assert_eq!(
            resolve(Some(ip("::ffff:127.0.0.1")), &h, &loopback(), true),
            Some(ip("203.0.113.9"))
        );
    }
}
