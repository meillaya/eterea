//! Shared date parsing for JSON/archive import sources.

use crate::models::Bookmark;
use crate::Result;
use chrono::{DateTime, Utc};

/// Earliest plausible saved-post timestamp: X launched in March 2006, so no
/// export can contain a post older than this. Used to reject a numeric
/// timestamp whose unit was guessed wrong instead of storing an absurd year.
const EARLIEST_X_POST_SECONDS: i64 = 1_141_171_200; // 2006-03-01T00:00:00Z

/// Tolerance for future timestamps (clock skew, scheduled posts). A numeric
/// timestamp more than a year ahead is treated as an unsupported unit.
const FUTURE_TOLERANCE_SECONDS: i64 = 366 * 24 * 60 * 60;

/// Decimal magnitude boundaries for unix timestamps. Values below `1e11` are
/// seconds, below `1e14` milliseconds and below `1e17` microseconds; the
/// counts keep an extra `1e3` gap so 10/13/16-digit values are unambiguous.
const MS_MAGNITUDE: i64 = 100_000_000_000; // 1e11
const US_MAGNITUDE: i64 = 100_000_000_000_000; // 1e14
const NS_MAGNITUDE: i64 = 100_000_000_000_000_000; // 1e17

/// Parse the first usable timestamp from up to three candidate strings.
///
/// Accepts RFC 3339, the classic `Wed Oct 10 20:19:24 +0000 2018` Twitter
/// format, and bare integers in unix seconds, milliseconds or microseconds
/// (unit chosen by decimal magnitude, see [`parse_epoch_candidate`]). Missing
/// candidates are skipped; an entirely unusable set is an import error for the
/// entry.
pub(crate) fn parse_date_candidates(candidates: [Option<&str>; 3]) -> Result<DateTime<Utc>> {
    for candidate in candidates.into_iter().flatten() {
        let s = candidate.trim().trim_matches('"');
        if let Ok(epoch) = s.parse::<i64>() {
            if let Some(dt) = parse_epoch_candidate(epoch) {
                return Ok(dt);
            }
        }
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Ok(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = DateTime::parse_from_str(s, "%a %b %d %H:%M:%S %z %Y") {
            return Ok(dt.with_timezone(&Utc));
        }
    }
    Err(crate::Error::Other("Could not parse date".into()))
}

/// Interpret one bare-integer candidate as a unix timestamp.
///
/// Unit policy: the unit is chosen from the decimal magnitude — values below
/// `1e11` are seconds (10 digits), below `1e14` milliseconds (13 digits) and
/// below `1e17` microseconds (16 digits). The converted instant must fall in
/// the plausible saved-post window (`2006-03-01T00:00:00Z ..= now + 1 year`);
/// anything else (pre-X dates, year > 9999 artefacts, nanoseconds) returns
/// `None` so the row is rejected instead of storing year 56302. This makes a
/// JSON export that uses `Date.getTime()`-style millisecond epochs import with
/// the true date, while a wrong/unsupported unit fails the row.
pub(crate) fn parse_epoch_candidate(value: i64) -> Option<DateTime<Utc>> {
    if value <= 0 {
        return None;
    }
    let (seconds, nanos) = if value < MS_MAGNITUDE {
        (value, 0)
    } else if value < US_MAGNITUDE {
        (
            value.div_euclid(1_000),
            (value.rem_euclid(1_000) as u32) * 1_000_000,
        )
    } else if value < NS_MAGNITUDE {
        (
            value.div_euclid(1_000_000),
            (value.rem_euclid(1_000_000) as u32) * 1_000,
        )
    } else {
        return None;
    };
    let timestamp = DateTime::<Utc>::from_timestamp(seconds, nanos)?;
    let latest = Utc::now()
        .timestamp()
        .saturating_add(FUTURE_TOLERANCE_SECONDS);
    if timestamp.timestamp() < EARLIEST_X_POST_SECONDS || timestamp.timestamp() > latest {
        return None;
    }
    Some(timestamp)
}

/// Canonical X status URL for a handle/id pair.
///
/// An empty handle falls back to the handle-less web status URL so the quote
/// keeps a usable link without inventing an author.
pub(crate) fn tweet_url_for(handle: &str, tweet_id: &str) -> String {
    let handle = handle.trim().trim_start_matches('@');
    if handle.is_empty() || handle == "i" {
        format!("https://x.com/i/web/status/{tweet_id}")
    } else {
        format!("https://x.com/{handle}/status/{tweet_id}")
    }
}

/// Fill tags from the post text when the export had no explicit tag list.
pub(crate) fn fill_extracted_hashtags(bookmark: &mut Bookmark) {
    if bookmark.tags.is_empty() {
        for tag in bookmark.extract_hashtags() {
            if !bookmark.tags.contains(&tag) {
                bookmark.tags.push(tag);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn parse_one(value: &str) -> Result<DateTime<Utc>> {
        parse_date_candidates([Some(value), None, None])
    }

    #[test]
    fn parses_seconds_milliseconds_and_microseconds_to_the_same_instant() {
        let expected = Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap();
        for candidate in ["1714564800", "1714564800000", "1714564800000000"] {
            assert_eq!(parse_one(candidate).unwrap(), expected, "{candidate}");
        }
    }

    #[test]
    fn keeps_existing_string_formats() {
        let expected = Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap();
        assert_eq!(parse_one("2024-05-01T12:00:00Z").unwrap(), expected);
        assert_eq!(
            parse_one("\"2024-05-01T12:00:00+00:00\"").unwrap(),
            expected
        );
        let legacy = parse_one("Wed Oct 10 20:19:24 +0000 2018").unwrap();
        assert_eq!(legacy.format("%Y-%m-%d").to_string(), "2018-10-10");
    }

    #[test]
    fn rejects_implausible_numeric_timestamps_instead_of_storing_absurd_years() {
        // i64::MAX as "seconds" would be year 292 billion; a 2286 second value
        // is numerically valid but outside the plausible X-post window.
        for candidate in [
            i64::MAX.to_string(),
            "999999999999999999".to_string(),
            "9999999999".to_string(),
            "0".to_string(),
            "-1".to_string(),
        ] {
            assert!(
                parse_one(&candidate).is_err(),
                "{candidate} must be rejected"
            );
        }
    }

    #[test]
    fn nanosecond_magnitude_is_unsupported_and_rejected() {
        // 1.7145648e18 ns = 1714564800000000000; only s/ms/us are supported.
        assert!(parse_one("1714564800000000000").is_err());
    }
}
