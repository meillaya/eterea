//! Pure logic shared by the app and the verification harnesses.
//!
//! This crate performs no I/O. It is the target for the Verus/Kani proofs.

#![forbid(unsafe_code)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Byte ranges of plain-text links, hashtags and mentions, in source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Link,
    Hashtag,
    Mention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// True when a URL is safe to request or open: HTTPS, a real host, no
/// credentials, no fragment, no control characters, and no private, loopback,
/// link-local, multicast or otherwise non-public address.
///
/// DNS is not resolved here. A public host name that resolves to a private
/// address is a separate failure mode; the media fetcher resolves normally and
/// documents that limitation.
pub fn is_safe_https_url(input: &str) -> bool {
    if input.is_empty() || input.len() > 4096 {
        return false;
    }
    if input.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Some((scheme, rest)) = input.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") || rest.is_empty() {
        return false;
    }
    if input.contains('#') {
        return false;
    }
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return false;
    }

    let (host, port) = match authority.strip_prefix('[') {
        Some(inner) => {
            let Some(end) = inner.find(']') else {
                return false;
            };
            let host = &inner[..end];
            let after = &inner[end + 1..];
            let port = if after.is_empty() {
                None
            } else {
                let Some(port) = after.strip_prefix(':') else {
                    return false;
                };
                Some(port)
            };
            (host, port)
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            Some(_) => return false,
            None => (authority, None),
        },
    };

    if let Some(port) = port {
        if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        match port.parse::<u32>() {
            Ok(0) | Err(_) => return false,
            Ok(1..=65535) => {}
            Ok(_) => return false,
        }
    }

    let host = host.trim_end_matches('.');
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return false;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => is_public_v4(v4),
        Ok(IpAddr::V6(v6)) => is_public_v6(v6),
        Err(_) => match legacy_ipv4(host) {
            // A numeric host that is not canonical IPv4 can still resolve to
            // loopback in common resolvers (inet_aton forms). Treat it as an
            // address, never as a hostname.
            Some(v4) => is_public_v4(v4),
            None => host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_'),
        },
    }
}

/// Parse inet_aton-style numeric IPv4 hosts: `127.1`, `2130706433`,
/// `0x7f.0.0.1`, octal parts. Returns `Some` for a host made only of numeric
/// parts; invalid numeric combinations return the unspecified address so the
/// public-address check rejects them. Non-numeric hosts return `None`.
fn legacy_ipv4(host: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 || parts.iter().any(|part| part.is_empty()) {
        return if parts.iter().all(|p| p.is_empty()) {
            None
        } else {
            Some(Ipv4Addr::UNSPECIFIED)
        };
    }
    let mut values = Vec::with_capacity(parts.len());
    for part in &parts {
        let (radix, digits) =
            if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
                (16, hex)
            } else if part.len() > 1 && part.starts_with('0') {
                (8, &part[1..])
            } else {
                (10, *part)
            };
        if digits.is_empty() {
            return Some(Ipv4Addr::UNSPECIFIED);
        }
        let Ok(value) = u32::from_str_radix(digits, radix) else {
            return None;
        };
        values.push(value);
    }
    if values.len() == 1 {
        return Some(Ipv4Addr::from(values[0]));
    }
    let last_index = values.len() - 1;
    let mut address: u32 = 0;
    for (index, value) in values.iter().enumerate() {
        if index == last_index {
            let remaining_bits = 8 * (4 - last_index);
            if *value >= (1u32 << remaining_bits) {
                return Some(Ipv4Addr::UNSPECIFIED);
            }
            address |= *value;
        } else {
            if *value > 0xff {
                return Some(Ipv4Addr::UNSPECIFIED);
            }
            address |= *value << (8 * (3 - index));
        }
    }
    Some(Ipv4Addr::from(address))
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    !(v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4.is_unspecified()
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)))
}

fn is_public_v6(v6: Ipv6Addr) -> bool {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let segments = v6.segments();
    !(v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xfe00) == 0xfc00)
}

/// True when a media MIME type is an image eterea can decode.
pub fn is_image_mime(mime: &str) -> bool {
    let essence = mime.split(';').next().unwrap_or("").trim();
    matches!(
        essence.to_ascii_lowercase().as_str(),
        "image/jpeg" | "image/png" | "image/gif" | "image/webp" | "image/avif"
    )
}

/// Classification of one external URL entity attached to a post.
///
/// The data layer stores the same three kinds; this crate stays dependency-free
/// so the classification can be model-checked. The storage model converts
/// [`LinkKind`] into its serde representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// Another X/Twitter post (a status URL or a quote link).
    Tweet,
    /// A media file on an X media host or an image/video file extension.
    Media,
    /// Any other external destination.
    External,
}

/// Classify a URL entity from a post.
///
/// `expanded_url` is the resolved destination X puts in entities; either the
/// compact `url` or the expanded form may match, so both are considered. The
/// classification is deterministic and purely syntactic: no DNS, no network.
pub fn classify_link(url: &str, expanded_url: Option<&str>) -> LinkKind {
    let candidates = [Some(url), expanded_url].into_iter().flatten();
    let mut kind = LinkKind::External;
    for candidate in candidates {
        if is_tweet_status_url(candidate) {
            return LinkKind::Tweet;
        }
        if is_media_url(candidate) {
            kind = LinkKind::Media;
        }
    }
    kind
}

/// True for an X/Twitter status URL: an `x.com`/`twitter.com` host and a
/// `/status/` path segment.
fn is_tweet_status_url(url: &str) -> bool {
    let Some(host) = url_host(url) else {
        return false;
    };
    let is_x_host = host == "x.com"
        || host == "twitter.com"
        || host.ends_with(".x.com")
        || host.ends_with(".twitter.com");
    is_x_host && url.contains("/status/")
}

/// True for a media host or a URL whose path ends in an image/video file
/// extension. A status URL is handled before this check.
fn is_media_url(url: &str) -> bool {
    if let Some(host) = url_host(url) {
        if matches!(
            host,
            "pbs.twimg.com" | "video.twimg.com" | "pic.twitter.com" | "ton.twimg.com"
        ) {
            return true;
        }
    }
    let path = url
        .split_once("://")
        .map(|(_, rest)| rest.split_once('/').map(|(_, path)| path).unwrap_or(""))
        .unwrap_or(url);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let lower = path.to_ascii_lowercase();
    [
        ".jpg", ".jpeg", ".png", ".gif", ".webp", ".avif", ".mp4", ".m4v", ".mov",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension))
}

/// Lowercase host of an absolute URL, without credentials or port.
fn url_host(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = match authority.strip_prefix('[') {
        Some(_) => return None,
        None => authority
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(authority),
    };
    if host.is_empty() {
        return None;
    }
    Some(host)
}

/// One progressive video variant, borrowed from a parsed media payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoVariant<'a> {
    pub url: &'a str,
    /// `content_type` from the payload, when present (`video/mp4`, `application/x-mpegURL`, ...).
    pub content_type: Option<&'a str>,
    /// Declared bit rate in bits per second.
    pub bit_rate: Option<u64>,
}

/// True when a variant is a progressive MP4 that a local player can open
/// directly: a `video/mp4` content type (or an `.mp4` URL when the payload
/// omitted the content type) and never an HLS playlist.
pub fn is_progressive_mp4_variant(variant: &VideoVariant<'_>) -> bool {
    let url = variant.url.to_ascii_lowercase();
    if url.contains(".m3u8") {
        return false;
    }
    match variant.content_type {
        Some(content_type) => {
            let essence = content_type.split(';').next().unwrap_or("").trim();
            essence.eq_ignore_ascii_case("video/mp4")
        }
        None => url.split(['?', '#']).next().unwrap_or("").ends_with(".mp4"),
    }
}

/// Best progressive MP4 playback variant: highest bit rate, `video/mp4` only,
/// no HLS. Ties break on the lexicographically smallest URL so the choice is
/// deterministic for the same payload.
pub fn best_mp4_variant<'a>(variants: &'a [VideoVariant<'a>]) -> Option<&'a VideoVariant<'a>> {
    variants
        .iter()
        .filter(|variant| is_progressive_mp4_variant(variant))
        .max_by(|left, right| {
            left.bit_rate
                .unwrap_or(0)
                .cmp(&right.bit_rate.unwrap_or(0))
                .then_with(|| right.url.cmp(left.url))
        })
}

/// The best progressive MP4 URL, if any variant qualifies.
pub fn best_mp4_playback_url<'a>(variants: &'a [VideoVariant<'a>]) -> Option<&'a str> {
    best_mp4_variant(variants).map(|variant| variant.url)
}

/// True when a tag color is exactly a `#rrggbb` hex triplet.
pub fn is_valid_hex_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_hosts() {
        assert!(is_safe_https_url("https://pbs.twimg.com/media/x.jpg"));
        assert!(is_safe_https_url("https://x.com/a/status/1"));
        assert!(is_safe_https_url("https://example.com:8443/x"));
        assert!(is_safe_https_url("HTTPS://example.com/x"));
        assert!(is_safe_https_url("https://[2606:4700:4700::1111]/x"));
    }

    #[test]
    fn rejects_unsafe_targets() {
        for url in [
            "http://pbs.twimg.com/x.jpg",
            "https://user:pass@x.com/x",
            "https://127.0.0.1/x",
            "https://127.0.0.1?x",
            "https://localhost/x",
            "https://a.localhost/x",
            "https://10.0.0.1/x",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/x",
            "https://198.18.0.1/x",
            "https://0.0.0.0/x",
            "https://224.0.0.1/x",
            "https://[::1]/x",
            "https://[fe80::1]/x",
            "https://[fc00::1]/x",
            "https://[::ffff:127.0.0.1]/x",
            "https://example.com/#frag",
            "https://example.com/a b",
            "https:///x",
            "https://example.com:0/x",
            "https://example.com:99999/x",
            "https://127.1/x",
            "https://2130706433/",
            "https://0x7f.0.0.1/",
            "https://0177.0.0.1/",
            "https://1.2.3.999/",
        ] {
            assert!(!is_safe_https_url(url), "should reject {url}");
        }
    }

    #[test]
    fn image_mimes() {
        assert!(is_image_mime("image/jpeg"));
        assert!(is_image_mime("image/png; charset=binary"));
        assert!(!is_image_mime("text/html"));
        assert!(!is_image_mime("video/mp4"));
    }

    #[test]
    fn classifies_tweet_media_and_external_links() {
        assert_eq!(
            classify_link("https://t.co/abc", Some("https://x.com/ada/status/1")),
            LinkKind::Tweet
        );
        assert_eq!(
            classify_link("https://x.com/ada/status/1", None),
            LinkKind::Tweet
        );
        assert_eq!(
            classify_link("https://mobile.twitter.com/ada/status/1", None),
            LinkKind::Tweet
        );
        assert_eq!(
            classify_link("https://t.co/pic", Some("https://pic.twitter.com/xyz")),
            LinkKind::Media
        );
        assert_eq!(
            classify_link("https://pbs.twimg.com/media/x.jpg", None),
            LinkKind::Media
        );
        assert_eq!(
            classify_link("https://example.com/photo.webp?size=large", None),
            LinkKind::Media
        );
        assert_eq!(
            classify_link("https://example.com/article", None),
            LinkKind::External
        );
        assert_eq!(
            classify_link("https://t.co/abc", Some("https://example.com/post")),
            LinkKind::External
        );
        // A bare `i/web` status URL still classifies as a tweet.
        assert_eq!(
            classify_link("https://x.com/i/web/status/42", None),
            LinkKind::Tweet
        );
    }

    fn variant<'a>(
        url: &'a str,
        content_type: Option<&'a str>,
        bit_rate: Option<u64>,
    ) -> VideoVariant<'a> {
        VideoVariant {
            url,
            content_type,
            bit_rate,
        }
    }

    #[test]
    fn best_mp4_requires_mp4_and_rejects_hls() {
        let variants = [
            variant(
                "https://video.twimg.com/a/hls.m3u8",
                Some("application/x-mpegURL"),
                Some(9_999_999),
            ),
            variant(
                "https://video.twimg.com/a/320.mp4",
                Some("video/mp4"),
                Some(320_000),
            ),
            variant(
                "https://video.twimg.com/a/720.mp4",
                Some("video/mp4"),
                Some(2_176_000),
            ),
            variant(
                "https://video.twimg.com/a/1080.webm",
                Some("video/webm"),
                Some(9_000_000),
            ),
        ];

        assert_eq!(
            best_mp4_playback_url(&variants),
            Some("https://video.twimg.com/a/720.mp4"),
            "highest bitrate MP4 wins and HLS never qualifies"
        );
    }

    #[test]
    fn best_mp4_is_deterministic_and_handles_missing_metadata() {
        let no_content_type = [
            variant("https://video.twimg.com/a/b.mp4", None, Some(1_000)),
            variant("https://video.twimg.com/a/a.mp4", None, Some(1_000)),
        ];
        assert_eq!(
            best_mp4_playback_url(&no_content_type),
            Some("https://video.twimg.com/a/a.mp4"),
            "ties break on the lexicographically smallest URL"
        );

        let none = [
            variant(
                "https://video.twimg.com/a/hls.m3u8",
                Some("application/x-mpegURL"),
                Some(5_000),
            ),
            variant("https://video.twimg.com/a/clip.m3u8", None, Some(5_000)),
        ];
        assert_eq!(best_mp4_playback_url(&none), None);
        assert_eq!(best_mp4_playback_url(&[]), None);
    }

    #[test]
    fn hex_colors_are_validated_strictly() {
        assert!(is_valid_hex_color("#000000"));
        assert!(is_valid_hex_color("#B4452F"));
        assert!(!is_valid_hex_color("b4452f"));
        assert!(!is_valid_hex_color("#fff"));
        assert!(!is_valid_hex_color("#b4452f "));
        assert!(!is_valid_hex_color("#gggggg"));
        assert!(!is_valid_hex_color(""));
    }
}
