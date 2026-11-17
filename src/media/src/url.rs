//! URL policy for media fetches.
//!
//! [`is_allowed_media_url`] is the single gate used by the manager, the HTTP
//! client and the video scaffold. It always calls `eterea_logic`'s predicate
//! first so the shared policy remains the source of truth, then applies the
//! stricter rules the media crate needs:
//!
//! - fragments are rejected (the shared predicate strips them instead of
//!   failing),
//! - whitespace and control characters are rejected,
//! - IPv6 link-local and unique-local addresses are rejected (the shared
//!   predicate only covers IPv6 loopback and unspecified addresses),
//! - IPv4 broadcast, multicast, `0.0.0.0/8` and carrier-grade NAT are
//!   rejected,
//! - `localhost` aliases (`localhost.`, `*.localhost`) are rejected.

use std::net::IpAddr;

/// Return `true` when `url` may be fetched by the media pipeline.
///
/// This is the only URL policy decision in the crate; it is called before a
/// request is queued, before every redirect hop, and again by the video
/// scaffold before it downloads.
///
/// ```
/// use eterea_media::is_allowed_media_url;
///
/// assert!(is_allowed_media_url("https://pbs.twimg.com/media/x.jpg"));
/// assert!(!is_allowed_media_url("http://pbs.twimg.com/media/x.jpg"));
/// assert!(!is_allowed_media_url("https://127.0.0.1/x.jpg"));
/// assert!(!is_allowed_media_url("https://pbs.twimg.com/media/x.jpg#frag"));
/// ```
pub fn is_allowed_media_url(url: &str) -> bool {
    // The shared predicate is always consulted first: it is the project-wide
    // definition of a safe HTTPS URL and the proof target in verification/.
    if !eterea_logic::is_safe_https_url(url) {
        return false;
    }
    if url.contains('#') {
        return false;
    }
    if url.is_empty()
        || url
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return false;
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    // Stop at any URI delimiter, not only '/'; a query can follow the host
    // directly (`https://127.0.0.1?x`), which a naive split would miss.
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let host = authority
        .strip_prefix('[')
        .and_then(|value| value.split(']').next())
        .unwrap_or_else(|| authority.split(':').next().unwrap_or(authority));
    let host = host.trim_end_matches('.');
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return false;
    }
    if host.to_ascii_lowercase().ends_with(".localhost") {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) => is_public_address(ip),
        Err(_) => true,
    }
}

/// Return `true` for addresses the media pipeline is allowed to contact.
///
/// This is stricter than [`std::net::Ipv4Addr::is_private`] because it also
/// covers link-local, broadcast, multicast, this-network and CGNAT ranges, and
/// the IPv6 equivalents.
pub(crate) fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
            {
                return false;
            }
            let octets = v4.octets();
            // 0.0.0.0/8: "this network", never a remote media host.
            if octets[0] == 0 {
                return false;
            }
            // 100.64.0.0/10: carrier-grade NAT space.
            if octets[0] == 100 && (64..=127).contains(&octets[1]) {
                return false;
            }
            true
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            let first = v6.segments()[0];
            // fe80::/10 link-local and fc00::/7 unique-local.
            if (first & 0xffc0) == 0xfe80 || (first & 0xfe00) == 0xfc00 {
                return false;
            }
            // IPv4-mapped addresses inherit the IPv4 policy (::ffff:127.0.0.1).
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_address(IpAddr::V4(v4));
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_allowed_media_url, is_public_address};
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn is_public_v4(text: &str) -> bool {
        is_public_address(IpAddr::V4(text.parse::<Ipv4Addr>().unwrap()))
    }

    fn is_public_v6(text: &str) -> bool {
        is_public_address(IpAddr::V6(text.parse::<Ipv6Addr>().unwrap()))
    }

    use std::net::IpAddr;

    #[test]
    fn accepts_public_https_urls() {
        for url in [
            "https://pbs.twimg.com/media/x.jpg",
            "https://x.com/a/status/1",
            "https://media.example.com:8443/path/to/file.png?format=jpg&name=small",
            "https://example.com",
        ] {
            assert!(is_allowed_media_url(url), "{url}");
        }
    }

    #[test]
    fn rejects_non_https() {
        for url in [
            "http://pbs.twimg.com/x.jpg",
            "ftp://media.example.com/x.jpg",
            "//pbs.twimg.com/x.jpg",
            "pbs.twimg.com/x.jpg",
            "",
        ] {
            assert!(!is_allowed_media_url(url), "{url}");
        }
    }

    #[test]
    fn rejects_credentials_and_fragments() {
        for url in [
            "https://user:pass@x.com/x.jpg",
            "https://user@x.com/x.jpg",
            "https://x.com/a#fragment",
            "https://pbs.twimg.com/media/x.jpg#",
        ] {
            assert!(!is_allowed_media_url(url), "{url}");
        }
    }

    #[test]
    fn rejects_whitespace_and_control_characters() {
        for url in [
            "https://x.com/a b.jpg",
            "https://x.com/a\n.jpg",
            "https://x.com/a\u{0000}b.jpg",
        ] {
            assert!(!is_allowed_media_url(url), "{url:?}");
        }
    }

    #[test]
    fn rejects_loopback_and_localhost_aliases() {
        for url in [
            "https://127.0.0.1/x.jpg",
            "https://127.1.2.3/x.jpg",
            "https://localhost/x.jpg",
            "https://LOCALHOST/x.jpg",
            "https://localhost./x.jpg",
            "https://media.localhost/x.jpg",
            "https://[::1]/x.jpg",
            // Query directly after the host must not bypass the host check.
            "https://localhost?x=1",
            "https://127.0.0.1?x=1",
            "https://[::1]?x=1",
            "https://media.localhost?x=1",
        ] {
            assert!(!is_allowed_media_url(url), "{url}");
        }
    }

    #[test]
    fn rejects_private_and_link_local_hosts() {
        for url in [
            "https://10.0.0.1/x.jpg",
            "https://192.168.1.20/x.jpg",
            "https://172.16.0.5/x.jpg",
            "https://169.254.169.254/latest/meta-data",
            "https://0.0.0.0/x.jpg",
            "https://255.255.255.255/x.jpg",
            "https://224.0.0.1/x.jpg",
            "https://100.64.0.1/x.jpg",
            "https://10.0.0.1?tracking=1",
            "https://169.254.169.254?latest",
            "https://[fe80::1]/x.jpg",
            "https://[fc00::1]/x.jpg",
            "https://[::ffff:127.0.0.1]/x.jpg",
            "https://[::]/x.jpg",
        ] {
            assert!(!is_allowed_media_url(url), "{url}");
        }
    }

    #[test]
    fn public_address_predicate_covers_the_ranges() {
        assert!(is_public_v4("93.184.216.34"));
        assert!(is_public_v4("8.8.8.8"));
        assert!(!is_public_v4("10.1.2.3"));
        assert!(!is_public_v4("172.31.255.255"));
        assert!(!is_public_v4("192.168.0.1"));
        assert!(!is_public_v4("169.254.1.1"));
        assert!(!is_public_v4("127.0.0.1"));
        assert!(!is_public_v4("0.1.2.3"));
        assert!(!is_public_v4("100.127.255.255"));
        assert!(is_public_v4("100.128.0.0"));

        assert!(is_public_v6("2606:4700:4700::1111"));
        assert!(!is_public_v6("::1"));
        assert!(!is_public_v6("::"));
        assert!(!is_public_v6("fe80::1"));
        assert!(!is_public_v6("fc00::1"));
        assert!(!is_public_v6("fd12:3456::1"));
        assert!(!is_public_v6("ff02::1"));
        assert!(!is_public_v6("::ffff:192.168.0.1"));
        assert!(is_public_v6("::ffff:8.8.8.8"));
    }

    #[test]
    fn shared_predicate_is_consulted() {
        // Both layers reject link-local addresses and fragments. Keep the
        // relationship explicit so a later relaxation cannot slip through.
        assert!(!eterea_logic::is_safe_https_url("https://[fe80::1]/x.jpg"));
        assert!(!is_allowed_media_url("https://[fe80::1]/x.jpg"));
        assert!(!eterea_logic::is_safe_https_url(
            "https://pbs.twimg.com/x.jpg#frag"
        ));
        assert!(!is_allowed_media_url("https://pbs.twimg.com/x.jpg#frag"));
    }
}
