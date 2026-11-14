//! Minimal CIDR matching for the trusted-proxy allowlist.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
};

/// An IP network such as `10.0.0.0/8` or `fd00::/8`. A bare address is a host route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix: u8,
}

/// The text was not a valid address or CIDR range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CidrParseError;

impl Cidr {
    /// Returns whether `ip` lies inside this network. Families never match each other.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                mask_v4(self.prefix) & u32::from(network) == mask_v4(self.prefix) & u32::from(ip)
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                mask_v6(self.prefix) & u128::from(network) == mask_v6(self.prefix) & u128::from(ip)
            }
            _ => false,
        }
    }
}

/// Builds the IPv4 netmask for a prefix length of 0..=32.
fn mask_v4(prefix: u8) -> u32 {
    u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
}

/// Builds the IPv6 netmask for a prefix length of 0..=128.
fn mask_v6(prefix: u8) -> u128 {
    u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0)
}

impl FromStr for Cidr {
    type Err = CidrParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (value, None),
        };
        let network = IpAddr::from_str(address.trim()).map_err(|_| CidrParseError)?;
        let max = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix = match prefix {
            Some(prefix) => prefix.trim().parse::<u8>().map_err(|_| CidrParseError)?,
            None => max,
        };
        if prefix > max {
            return Err(CidrParseError);
        }
        // Zero the host bits so equality and matching are canonical.
        let network = match network {
            IpAddr::V4(ip) => IpAddr::V4(Ipv4Addr::from(u32::from(ip) & mask_v4(prefix))),
            IpAddr::V6(ip) => IpAddr::V6(Ipv6Addr::from(u128::from(ip) & mask_v6(prefix))),
        };
        Ok(Self { network, prefix })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cidr(value: &str) -> Cidr {
        value.parse().unwrap()
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn matches_ranges_and_hosts() {
        assert!(cidr("10.0.0.0/8").contains(ip("10.200.1.2")));
        assert!(!cidr("10.0.0.0/8").contains(ip("11.0.0.1")));
        assert!(cidr("192.168.1.5").contains(ip("192.168.1.5")));
        assert!(!cidr("192.168.1.5").contains(ip("192.168.1.6")));
        assert!(cidr("fd00::/8").contains(ip("fd12::1")));
        assert!(!cidr("fd00::/8").contains(ip("fe80::1")));
    }

    #[test]
    fn zero_prefix_matches_the_whole_family_only() {
        assert!(cidr("0.0.0.0/0").contains(ip("8.8.8.8")));
        assert!(!cidr("0.0.0.0/0").contains(ip("::1")));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "10.0.0.0/33",
            "::/129",
            "nope",
            "10.0.0.0/x",
            "10.0.0/8",
        ] {
            assert!(bad.parse::<Cidr>().is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn host_bits_are_normalised() {
        assert_eq!(cidr("10.1.2.3/8"), cidr("10.0.0.0/8"));
    }
}
