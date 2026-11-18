//! Parsing tests for the opt-in embed payload, using real captured payloads.
//!
//! The fixtures are verbatim responses from the public embed endpoint (field
//! spelling and shape included); they are data, not instructions.

use eterea_media::{parse_tweet_payload, SyndicatedMediaKind, SyndicationOutcome};

fn tweet(json: &str) -> eterea_media::SyndicatedTweet {
    match parse_tweet_payload(json).expect("payload parses") {
        SyndicationOutcome::Tweet(tweet) => *tweet,
        SyndicationOutcome::Unavailable => panic!("expected an available post"),
    }
}

#[test]
fn video_tweet_maps_variants_and_duration() {
    let tweet = tweet(include_str!("fixtures/tweet-video.json"));
    assert_eq!(tweet.id, "2122298644456198572");
    assert_eq!(tweet.author.handle, "blumotel");
    assert_eq!(tweet.author.name, "🎸");
    assert_eq!(tweet.media.len(), 1);
    let media = &tweet.media[0];
    assert_eq!(media.kind, SyndicatedMediaKind::Video);
    assert_eq!(media.duration_ms, Some(63_891));
    assert!(media.variants.iter().any(|variant| {
        variant.content_type.as_deref() == Some("video/mp4") && variant.bitrate == Some(1_280_000)
    }));
    assert!(tweet.quoted.is_none());
}

#[test]
fn quote_tweet_maps_quoted_carousel_videos() {
    let tweet = tweet(include_str!("fixtures/tweet-quote-videos.json"));
    assert_eq!(tweet.id, "2122399349695303865");
    assert!(tweet.media.is_empty());
    let quoted = tweet.quoted.expect("quoted post parsed");
    assert_eq!(quoted.author.handle, "ayomifilm");
    assert_eq!(quoted.media.len(), 5);
    assert!(quoted
        .media
        .iter()
        .all(|media| media.kind == SyndicatedMediaKind::Video));
    assert!(quoted
        .media
        .iter()
        .any(|media| media.duration_ms == Some(3_166)));
    assert!(quoted.media.iter().any(|media| media
        .variants
        .iter()
        .any(|variant| variant.content_type.as_deref() == Some("video/mp4"))));
}

#[test]
fn photo_carousel_maps_every_image() {
    let tweet = tweet(include_str!("fixtures/tweet-photo-carousel.json"));
    assert_eq!(tweet.media.len(), 4);
    assert!(tweet
        .media
        .iter()
        .all(|media| media.kind == SyndicatedMediaKind::Photo));
    assert!(tweet
        .media
        .iter()
        .all(|media| media.url.starts_with("https://pbs.twimg.com/media/")));
}

#[test]
fn tombstone_is_unavailable_not_an_error() {
    assert_eq!(
        parse_tweet_payload(include_str!("fixtures/tweet-tombstone.json")).expect("parses"),
        SyndicationOutcome::Unavailable
    );
}

#[test]
fn malformed_payload_is_an_error() {
    assert!(parse_tweet_payload("not json").is_err());
    assert!(parse_tweet_payload(r#"{"__typename":"Tweet"}"#).is_err());
}
