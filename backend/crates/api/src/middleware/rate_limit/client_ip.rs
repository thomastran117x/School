//! Trusted client IP resolution.
//!
//! The TCP peer address is the only value an attacker cannot forge, so it is the
//! default. `X-Forwarded-For` is consulted **only** when the peer itself is inside the
//! configured trusted proxy ranges (`TRUSTED_PROXY_CIDRS`). Then the header is read
//! from the right: each trusted proxy appends the address it received the request
//! from, so the first untrusted entry from the right is the real client. Everything to
//! its left is client-controlled and ignored.
//!
//! Anything unexpected (unparseable entries, oversized headers, too many hops) falls
//! back to the peer address, which makes the request share the proxy's bucket. That is
//! the strict, safe outcome. `X-Real-IP` and `Forwarded` are never read.
//!
//! IPv6 addresses are reduced to their /64 so a client cannot rotate through a
//! prefix it owns to dodge limits. IPv4-mapped IPv6 addresses become plain IPv4.

use std::net::{IpAddr, Ipv6Addr};

use axum::http::{HeaderMap, header::HeaderName};

use super::cidr::Cidr;

/// Header carrying the proxy chain.
const FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
/// Maximum accepted size of the combined header value, in bytes.
const MAX_HEADER_BYTES: usize = 2048;
/// Maximum number of hops examined.
const MAX_HOPS: usize = 32;

/// Resolves the client address used as the rate-limit subject.
#[derive(Clone, Debug, Default)]
pub struct ClientIpResolver {
    trusted: Vec<Cidr>,
}

impl ClientIpResolver {
    /// Creates a resolver trusting forwarded headers from the given proxy ranges.
    pub fn new(trusted: Vec<Cidr>) -> Self {
        Self { trusted }
    }

    fn is_trusted(&self, ip: IpAddr) -> bool {
        self.trusted.iter().any(|cidr| cidr.contains(ip))
    }

    /// Returns the normalised client address for a request.
    pub fn resolve(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        let peer = canonical(peer);
        if !self.is_trusted(peer) {
            return normalize(peer);
        }
        match self.forwarded_client(headers) {
            Some(client) => normalize(client),
            None => normalize(peer),
        }
    }

    /// Walks `X-Forwarded-For` from the right and returns the first untrusted hop.
    fn forwarded_client(&self, headers: &HeaderMap) -> Option<IpAddr> {
        let mut hops: Vec<IpAddr> = Vec::new();
        let mut total = 0_usize;
        for value in headers.get_all(FORWARDED_FOR) {
            total += value.len();
            if total > MAX_HEADER_BYTES {
                return None;
            }
            for part in value.to_str().ok()?.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    return None;
                }
                hops.push(canonical(part.parse::<IpAddr>().ok()?));
                if hops.len() > MAX_HOPS {
                    return None;
                }
            }
        }
        hops.into_iter().rev().find(|hop| !self.is_trusted(*hop))
    }
}

/// Converts IPv4-mapped IPv6 addresses to IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

/// Reduces IPv6 addresses to their /64 network so prefix rotation shares one bucket.
fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn resolver(trusted: &[&str]) -> ClientIpResolver {
        ClientIpResolver::new(trusted.iter().map(|c| c.parse().unwrap()).collect())
    }

    fn headers(values: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for value in values {
            map.append(FORWARDED_FOR, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn untrusted_peers_cannot_spoof_the_client_address() {
        let resolver = resolver(&["10.0.0.0/8"]);
        let got = resolver.resolve(ip("203.0.113.9"), &headers(&["1.2.3.4"]));
        assert_eq!(got, ip("203.0.113.9"));
    }

    #[test]
    fn empty_trust_list_always_uses_the_peer() {
        let got = resolver(&[]).resolve(ip("10.0.0.1"), &headers(&["1.2.3.4"]));
        assert_eq!(got, ip("10.0.0.1"));
    }

    #[test]
    fn trusted_peer_yields_the_first_untrusted_hop_from_the_right() {
        let resolver = resolver(&["10.0.0.0/8"]);
        // The client forged 9.9.9.9; the proxy appended the real address.
        let got = resolver.resolve(ip("10.0.0.1"), &headers(&["9.9.9.9, 198.51.100.7"]));
        assert_eq!(got, ip("198.51.100.7"));
    }

    #[test]
    fn chained_trusted_proxies_are_skipped_and_headers_are_joined() {
        let resolver = resolver(&["10.0.0.0/8"]);
        let got = resolver.resolve(
            ip("10.0.0.1"),
            &headers(&["9.9.9.9", "198.51.100.7, 10.0.0.5"]),
        );
        assert_eq!(got, ip("198.51.100.7"));
    }

    #[test]
    fn malformed_oversized_or_missing_headers_fall_back_to_the_peer() {
        let resolver = resolver(&["10.0.0.0/8"]);
        let peer = ip("10.0.0.1");
        let too_many = vec!["1.1.1.1"; 40].join(",");
        let oversized = "1.1.1.1,".repeat(400);
        for bad in [
            "garbage",
            "1.1.1.1,,2.2.2.2",
            "1.1.1.1, nope",
            &too_many,
            &oversized,
        ] {
            assert_eq!(resolver.resolve(peer, &headers(&[bad])), peer, "{bad:?}");
        }
        assert_eq!(resolver.resolve(peer, &HeaderMap::new()), peer);
    }

    #[test]
    fn an_all_trusted_chain_uses_the_peer_rather_than_a_claimed_address() {
        let resolver = resolver(&["10.0.0.0/8"]);
        let got = resolver.resolve(ip("10.0.0.1"), &headers(&["10.9.9.9, 10.0.0.2"]));
        assert_eq!(got, ip("10.0.0.1"));
    }

    #[test]
    fn ipv6_clients_share_a_bucket_per_64_and_mapped_v4_is_unwrapped() {
        let resolver = resolver(&[]);
        let a = resolver.resolve(ip("2001:db8:1:2:aaaa::1"), &HeaderMap::new());
        let b = resolver.resolve(ip("2001:db8:1:2:bbbb::9"), &HeaderMap::new());
        let c = resolver.resolve(ip("2001:db8:1:3::1"), &HeaderMap::new());
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(
            resolver.resolve(ip("::ffff:192.0.2.1"), &HeaderMap::new()),
            ip("192.0.2.1")
        );
    }
}
