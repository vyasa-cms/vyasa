//! Outbound request guard: the one place that decides whether the server
//! may open a connection to an address on someone else's behalf.
//!
//! Anything that fetches a URL a user, a plugin or post content supplied
//! (plugin `net:fetch`, the SEO link checker) goes through a client built
//! by [`guarded_builder`]. Two layers cover it:
//!
//! - a DNS resolver that refuses any name resolving to a loopback,
//!   private, link-local, unique-local, CGNAT, multicast, documentation or
//!   unspecified address. The connection is made to exactly the addresses
//!   the resolver checked, so a name cannot pass the check and then
//!   rebind to `127.0.0.1` for the connect (no second lookup happens);
//! - a redirect policy that re-checks every hop, because IP-literal hosts
//!   never reach a resolver.
//!
//! Proxies are disabled on guarded clients: through a proxy the resolver
//! would check the proxy's address, not the target's.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

/// Whether `ip` is an ordinary public unicast address the server may
/// connect to on someone else's behalf.
#[must_use]
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0                                   // "this network" / unspecified
        || a == 10                             // private
        || a == 127                            // loopback
        || (a == 100 && (64..128).contains(&b)) // CGNAT 100.64/10
        || (a == 169 && b == 254)              // link-local
        || (a == 172 && (16..32).contains(&b)) // private
        || (a == 192 && b == 0 && c == 0)      // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2)      // TEST-NET-1
        || (a == 192 && b == 88 && c == 99)    // 6to4 relay anycast
        || (a == 192 && b == 168)              // private
        || (a == 198 && (b == 18 || b == 19))  // benchmarking
        || (a == 198 && b == 51 && c == 100)   // TEST-NET-2
        || (a == 203 && b == 0 && c == 113)    // TEST-NET-3
        || a >= 224) // multicast, reserved, broadcast
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();
    // Addresses that carry an IPv4 address inside: judge the v4 part.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let embedded = |hi: u16, lo: u16| {
        let [h1, h2] = hi.to_be_bytes();
        let [l1, l2] = lo.to_be_bytes();
        Ipv4Addr::new(h1, h2, l1, l2)
    };
    // IPv4-compatible (deprecated) ::a.b.c.d, which also covers :: and ::1.
    if seg[..6].iter().all(|s| *s == 0) {
        return false;
    }
    // NAT64 64:ff9b::/96 and 64:ff9b:1::/48.
    if seg[0] == 0x64 && seg[1] == 0xff9b {
        return seg[2..6].iter().all(|s| *s == 0) && is_public_v4(embedded(seg[6], seg[7]));
    }
    // 6to4 2002:AABB:CCDD::/48 tunnels to the embedded v4 address.
    if seg[0] == 0x2002 {
        return is_public_v4(embedded(seg[1], seg[2]));
    }
    !((seg[0] & 0xfe00) == 0xfc00           // unique local fc00::/7
        || (seg[0] & 0xffc0) == 0xfe80      // link-local fe80::/10
        || (seg[0] & 0xffc0) == 0xfec0      // site-local (deprecated)
        || (seg[0] & 0xff00) == 0xff00      // multicast
        || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation
        || (seg[0] == 0x2001 && seg[1] == 0)      // Teredo (embeds anything)
        || (seg[0] == 0x0100 && seg[1..4].iter().all(|s| *s == 0))) // discard
}

/// Whether a URL is one a guarded client may be pointed at: http(s),
/// no userinfo, and — when the host is an address rather than a name —
/// a public one. Names are checked when they are resolved.
#[must_use]
pub fn url_allowed(url: &url::Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(d)) => !d.is_empty(),
        Some(url::Host::Ipv4(v4)) => is_public_v4(v4),
        Some(url::Host::Ipv6(v6)) => is_public_v6(v6),
        None => false,
    }
}

/// Resolver that refuses names pointing at internal addresses.
///
/// Any non-public address in the answer refuses the whole name: an
/// attacker-controlled zone mixing one public and one private record
/// should not get a connection attempt to the private one.
#[derive(Debug, Default, Clone, Copy)]
pub struct GuardedResolver;

/// The refusal a guarded lookup reports.
#[derive(Debug)]
struct Refused(String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} resolves to a non-public address", self.0)
    }
}

impl std::error::Error for Refused {}

/// Resolves `host` and returns its addresses only if every one is public.
///
/// # Errors
/// A message when the name does not resolve or resolves to any
/// non-public address.
pub async fn resolve_public(host: &str) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0))
        .await
        .map_err(|e| format!("{host}: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("{host}: no addresses"));
    }
    if addrs.iter().any(|a| !is_public_ip(a.ip())) {
        return Err(Refused(host.to_owned()).to_string());
    }
    Ok(addrs)
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            match resolve_public(&host).await {
                Ok(addrs) => Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs),
                Err(e) => Err(e.into()),
            }
        })
    }
}

/// A redirect policy that follows at most `max` hops, each of which must
/// pass [`url_allowed`] (a hop to a name is then checked at resolution).
#[must_use]
pub fn redirect_policy(max: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= max {
            attempt.error("too many redirects")
        } else if !url_allowed(attempt.url()) {
            attempt.error("redirect to a non-public address refused")
        } else {
            attempt.follow()
        }
    })
}

/// A client builder with the guarded resolver, `max_redirects` checked
/// hops, and no proxy. Callers add timeouts and a user agent.
pub fn guarded_builder(max_redirects: usize) -> reqwest::ClientBuilder {
    let policy = if max_redirects == 0 {
        reqwest::redirect::Policy::none()
    } else {
        redirect_policy(max_redirects)
    };
    reqwest::Client::builder()
        .dns_resolver(Arc::new(GuardedResolver))
        .redirect(policy)
        .no_proxy()
}

/// Why a capped body read failed.
#[derive(Debug)]
pub enum BodyError {
    /// The body exceeded the cap; the transfer was abandoned.
    TooLarge,
    /// The transport failed mid-body.
    Transport(reqwest::Error),
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => f.write_str("response too large"),
            Self::Transport(e) => write!(f, "{e}"),
        }
    }
}

/// Reads a response body, abandoning it as soon as more than `max` bytes
/// have arrived. The advertised `Content-Length` is checked first so an
/// honest oversized response is refused without reading anything, but it
/// is never trusted: the running count is what enforces the cap.
///
/// # Errors
/// [`BodyError::TooLarge`] past the cap, [`BodyError::Transport`] when
/// the connection fails.
pub async fn read_capped(
    mut response: reqwest::Response,
    max: usize,
) -> Result<Vec<u8>, BodyError> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(BodyError::TooLarge);
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(BodyError::Transport)? {
        if out.len() + chunk.len() > max {
            return Err(BodyError::TooLarge);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// A loopback server answering every connection with `reply`, then
    /// `chunks` chunked 1 KiB pieces when `chunks > 0`.
    async fn serve(reply: &'static [u8], chunks: usize) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                // Read the request first: closing a socket with unread
                // input resets the connection and truncates the reply.
                let mut buf = [0_u8; 4096];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(reply).await;
                if chunks > 0 {
                    for _ in 0..chunks {
                        let _ = sock.write_all(b"400\r\n").await;
                        let _ = sock.write_all(&[b'x'; 1024]).await;
                        let _ = sock.write_all(b"\r\n").await;
                    }
                    let _ = sock.write_all(b"0\r\n\r\n").await;
                }
                let _ = sock.shutdown().await;
            }
        });
        addr
    }

    #[test]
    fn internal_v4_addresses_are_refused() {
        for bad in [
            "0.0.0.0",
            "127.0.0.1",
            "127.255.0.9",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.127.255.255",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.2.5",
            "198.18.0.1",
        ] {
            assert!(!is_public_ip(ip(bad)), "{bad}");
        }
        for good in [
            "8.8.8.8",
            "1.1.1.1",
            "172.32.0.1",
            "100.128.0.1",
            "93.184.216.34",
        ] {
            assert!(is_public_ip(ip(good)), "{good}");
        }
    }

    #[test]
    fn internal_v6_addresses_are_refused_including_embedded_v4() {
        for bad in [
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "2002:7f00:1::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "2001:0::1",
        ] {
            assert!(!is_public_ip(ip(bad)), "{bad}");
        }
        for good in ["2606:4700:4700::1111", "::ffff:8.8.8.8", "2002:0808:0808::"] {
            assert!(is_public_ip(ip(good)), "{good}");
        }
    }

    #[test]
    fn url_literals_and_userinfo_are_checked() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert!(!url_allowed(&u("http://127.0.0.1/")));
        assert!(!url_allowed(&u("http://[::1]/")));
        assert!(!url_allowed(&u("http://[::ffff:7f00:1]/")));
        assert!(!url_allowed(&u("http://2130706433/")));
        assert!(!url_allowed(&u("http://0x7f.1/")));
        assert!(!url_allowed(&u("https://user@example.com/")));
        assert!(!url_allowed(&u("ftp://example.com/")));
        assert!(url_allowed(&u("https://example.com/")));
        assert!(url_allowed(&u("http://8.8.8.8/")));
    }

    #[tokio::test]
    async fn localhost_names_do_not_resolve_through_the_guard() {
        assert!(resolve_public("localhost").await.is_err());
    }

    #[tokio::test]
    async fn a_guarded_client_cannot_reach_a_loopback_server_by_name() {
        let addr = serve(
            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
            0,
        )
        .await;
        let by_name = format!("http://localhost:{}/", addr.port());
        // Control: an unguarded client does reach it.
        let plain = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(plain.get(&by_name).send().await.is_ok(), "control request");
        let guarded = guarded_builder(5).build().unwrap();
        assert!(guarded.get(&by_name).send().await.is_err());
    }

    #[tokio::test]
    async fn redirects_to_internal_addresses_are_refused() {
        // The first hop is loopback only because a test cannot stand up a
        // public server; what is under test is the policy on the second.
        let addr = serve(
            b"HTTP/1.1 302 Found\r\nlocation: http://169.254.169.254/latest/meta-data/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            0,
        )
        .await;
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(redirect_policy(5))
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap();
        let err = client.get(format!("http://{addr}/")).send().await;
        assert!(err.is_err_and(|e| e.is_redirect()));
    }

    #[tokio::test]
    async fn capped_reads_stop_at_the_cap_even_without_content_length() {
        // Chunked, so there is no Content-Length to pre-check.
        let addr = serve(
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
            64,
        )
        .await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = format!("http://{addr}/");
        let resp = client.get(&url).send().await.unwrap();
        assert!(matches!(
            read_capped(resp, 10 * 1024).await,
            Err(BodyError::TooLarge)
        ));
        let resp = client.get(&url).send().await.unwrap();
        assert_eq!(read_capped(resp, 64 * 1024).await.unwrap().len(), 64 * 1024);
    }
}
