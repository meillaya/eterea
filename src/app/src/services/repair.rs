//! Opt-in metadata repair from X's public embed payload.
//!
//! The reader can only show what an import carried. A text-only bookmarks CSV
//! leaves media, quotes and expanded links out, and older archives often lost
//! the avatar renditions X has since deleted. This pass fetches the public
//! embed payload for posts that have no media in the archive, maps it onto the
//! core model and merges it into the existing row. It is explicit, paced,
//! cancellable and never runs automatically: the endpoint is X's undocumented
//! embed backend, not an API contract.
//!
//! What it changes: media rows, quoted post, link entities, hashtags, note
//! text and the author's current name/avatar of **existing** rows only. It
//! never creates posts, never deletes, and never touches read/favorite state.
//! Media bytes are not downloaded here; the normal image/video cache fetches
//! them when the reader shows the post.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use eterea_core::{Bookmark, Database, LinkEntity, Media, MediaType, QuotedTweet};
use eterea_logic::VideoVariant;
use eterea_media::{
    tweet_id_from_url, SyndicatedMedia, SyndicatedMediaKind, SyndicatedQuote, SyndicatedTweet,
    SyndicationClient, SyndicationOutcome,
};

/// How much to repair and how fast.
#[derive(Debug, Clone, Copy)]
pub struct RepairOptions {
    /// Maximum number of posts to visit. `usize::MAX` means the whole archive.
    pub limit: usize,
    /// Pause between feed requests. 320 ms is roughly three payloads a second,
    /// which is deliberately gentle for an undocumented endpoint.
    pub delay: Duration,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self {
            limit: usize::MAX,
            delay: Duration::from_millis(320),
        }
    }
}

/// One progress tick from the repair pass.
#[derive(Debug, Clone, Copy, Default)]
pub struct RepairEvent {
    /// Posts visited so far.
    pub done: usize,
    /// Posts the pass planned to visit.
    pub total: usize,
    /// Rows that gained metadata.
    pub repaired: usize,
    /// Posts that no longer exist on X.
    pub unavailable: usize,
    /// Posts that failed for another reason (mapping, transport, database).
    pub failed: usize,
    /// The pass has finished (normally or cancelled).
    pub finished: bool,
    /// The pass stopped because cancellation was requested.
    pub cancelled: bool,
}

/// Final counts from one repair run.
#[derive(Debug, Clone, Copy, Default)]
pub struct RepairSummary {
    /// Posts visited.
    pub total: usize,
    /// Rows enriched.
    pub repaired: usize,
    /// Deleted/protected posts.
    pub unavailable: usize,
    /// Other failures.
    pub failed: usize,
    /// Cancellation was requested before the queue drained.
    pub cancelled: bool,
}

/// Repair missing media for the posts that have none.
///
/// Runs on the caller's thread (the desktop shell gives it a worker thread),
/// reports progress through `on_event`, and stops between posts when `cancel`
/// is set. Returns final counts.
pub fn repair_missing_media(
    db_path: &Path,
    options: RepairOptions,
    cancel: Arc<AtomicBool>,
    mut on_event: impl FnMut(RepairEvent),
) -> Result<RepairSummary> {
    let db = Database::open(db_path)
        .with_context(|| format!("failed to open database at {}", db_path.display()))?;
    let targets = db
        .bookmarks_missing_media(options.limit)
        .context("failed to list posts without media")?;
    let total = targets.len();
    let client = SyndicationClient::new()?;
    let mut summary = RepairSummary {
        total,
        ..RepairSummary::default()
    };
    let mut report = |summary: &RepairSummary, done: usize, finished: bool, cancelled: bool| {
        on_event(RepairEvent {
            done,
            total,
            repaired: summary.repaired,
            unavailable: summary.unavailable,
            failed: summary.failed,
            finished,
            cancelled,
        });
    };

    for (index, (bookmark_id, tweet_url)) in targets.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        report(&summary, index, false, false);

        let Some(tweet_id) = tweet_id_from_url(tweet_url) else {
            summary.failed += 1;
            sleep_unless_cancelled(options.delay, &cancel);
            continue;
        };
        match fetch_with_retry(&client, &tweet_id, &cancel) {
            Ok(SyndicationOutcome::Unavailable) => summary.unavailable += 1,
            Ok(SyndicationOutcome::Tweet(payload)) => {
                match bookmark_from_payload(bookmark_id, tweet_url, payload.as_ref()) {
                    Some(bookmark) => match db.enrich_bookmarks(&[bookmark]) {
                        Ok(1) => summary.repaired += 1,
                        Ok(_) => {}
                        Err(_) => summary.failed += 1,
                    },
                    None => summary.failed += 1,
                }
            }
            Err(_) => summary.failed += 1,
        }
        sleep_unless_cancelled(options.delay, &cancel);
    }

    summary.cancelled = summary.cancelled || cancel.load(Ordering::Relaxed);
    report(&summary, summary.total, true, summary.cancelled);
    Ok(summary)
}

fn fetch_with_retry(
    client: &SyndicationClient,
    tweet_id: &str,
    cancel: &AtomicBool,
) -> Result<SyndicationOutcome, eterea_media::MediaError> {
    let mut attempt = 0;
    loop {
        match client.fetch(tweet_id, cancel) {
            Ok(outcome) => return Ok(outcome),
            Err(error) => {
                let retryable = matches!(
                    error,
                    eterea_media::MediaError::HttpStatus(429)
                        | eterea_media::MediaError::HttpStatus(500)
                        | eterea_media::MediaError::HttpStatus(502)
                        | eterea_media::MediaError::HttpStatus(503)
                        | eterea_media::MediaError::HttpStatus(504)
                        | eterea_media::MediaError::Timeout
                        | eterea_media::MediaError::Connection
                );
                attempt += 1;
                if !retryable || attempt >= 3 || cancel.load(Ordering::Relaxed) {
                    return Err(error);
                }
                sleep_unless_cancelled(Duration::from_secs(2 * attempt as u64), cancel);
                if cancel.load(Ordering::Relaxed) {
                    return Err(eterea_media::MediaError::Cancelled);
                }
            }
        }
    }
}

fn sleep_unless_cancelled(duration: Duration, cancel: &AtomicBool) {
    let mut remaining = duration;
    while remaining > Duration::ZERO {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let step = remaining.min(Duration::from_millis(50));
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}

/// Convert an embed payload into the core model, keyed to the existing row.
fn bookmark_from_payload(
    existing_id: &str,
    tweet_url: &str,
    payload: &SyndicatedTweet,
) -> Option<Bookmark> {
    // The payload always carries created_at; without it the repair would have
    // to invent a date, so the row is left untouched.
    let tweeted_at = payload.created_at.as_deref().and_then(parse_rfc3339)?;
    Some(Bookmark {
        id: existing_id.to_string(),
        tweet_url: tweet_url.to_string(),
        content: payload.text.clone(),
        note_text: payload.note_text.clone(),
        tweeted_at,
        imported_at: Utc::now(),
        author_handle: payload.author.handle.clone(),
        author_name: payload.author.name.clone(),
        author_profile_url: None,
        author_profile_image: payload.author.avatar_url.clone(),
        tags: normalized_hashtags(&payload.hashtags),
        comments: None,
        media: payload.media.iter().map(map_media).collect(),
        quoted: payload.quoted.as_ref().and_then(map_quote),
        is_favorite: false,
        is_read: false,
        links: payload
            .links
            .iter()
            .map(|link| LinkEntity {
                url: link.url.clone(),
                expanded_url: link.expanded_url.clone(),
                display_url: link.display_url.clone(),
                kind: eterea_core::LinkKind::from(eterea_logic::classify_link(
                    &link.url,
                    link.expanded_url.as_deref(),
                )),
            })
            .collect(),
    })
}

fn normalized_hashtags(tags: &[String]) -> Vec<String> {
    let set: BTreeSet<String> = tags
        .iter()
        .map(|tag| tag.trim().trim_start_matches('#').to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    set.into_iter().collect()
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

fn map_media(media: &SyndicatedMedia) -> Media {
    let variants: Vec<VideoVariant<'_>> = media
        .variants
        .iter()
        .map(|variant| VideoVariant {
            url: &variant.url,
            content_type: variant.content_type.as_deref(),
            bit_rate: variant.bitrate,
        })
        .collect();
    let best = eterea_logic::best_mp4_playback_url(&variants).map(str::to_string);
    let (media_type, source_type) = match media.kind {
        SyndicatedMediaKind::Photo => (MediaType::Image, "photo"),
        SyndicatedMediaKind::Video => (MediaType::Video, "video"),
        SyndicatedMediaKind::AnimatedGif => (MediaType::Gif, "gif"),
    };
    let needs_poster = media.kind != SyndicatedMediaKind::Photo;
    let variants_json = (!media.variants.is_empty()).then(|| {
        let list: Vec<serde_json::Value> = media
            .variants
            .iter()
            .map(|variant| {
                serde_json::json!({
                    "content_type": variant.content_type,
                    "bitrate": variant.bitrate,
                    "url": variant.url,
                })
            })
            .collect();
        serde_json::Value::Array(list).to_string()
    });
    Media {
        url: media.url.clone(),
        media_type,
        alt_text: media.alt_text.clone(),
        width: media.width,
        height: media.height,
        source_media_key: None,
        source_type: Some(source_type.to_string()),
        preview_url: None,
        variant_url: best.clone(),
        variants_json,
        duration_ms: media.duration_ms,
        poster_url: needs_poster.then(|| media.url.clone()),
        playback_url: best,
    }
}

fn map_quote(quote: &SyndicatedQuote) -> Option<QuotedTweet> {
    if quote.text.trim().is_empty() && quote.author.handle.is_empty() {
        return None;
    }
    let mut quoted = QuotedTweet::new(
        format!("https://x.com/{}/status/{}", quote.author.handle, quote.id),
        quote.text.clone(),
        quote.created_at.as_deref().and_then(parse_rfc3339),
        quote.author.handle.clone(),
        quote.author.name.clone(),
    );
    quoted.author_profile_image = quote.author.avatar_url.clone();
    quoted.media = quote.media.iter().map(map_media).collect();
    Some(quoted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> SyndicatedTweet {
        SyndicatedTweet {
            id: "1".into(),
            text: "hello".into(),
            note_text: None,
            created_at: Some("2026-11-16T12:00:01.000Z".into()),
            author: eterea_media::SyndicatedAuthor {
                id: "9".into(),
                handle: "writer".into(),
                name: "Writer".into(),
                avatar_url: Some("https://pbs.twimg.com/profile_images/1/a_normal.jpg".into()),
            },
            media: vec![
                SyndicatedMedia {
                    kind: SyndicatedMediaKind::Photo,
                    url: "https://pbs.twimg.com/media/one.jpg".into(),
                    width: Some(1200),
                    height: Some(800),
                    duration_ms: None,
                    alt_text: Some("alt".into()),
                    variants: Vec::new(),
                },
                SyndicatedMedia {
                    kind: SyndicatedMediaKind::Video,
                    url: "https://pbs.twimg.com/thumb/poster.jpg".into(),
                    width: Some(720),
                    height: Some(720),
                    duration_ms: Some(5000),
                    alt_text: None,
                    variants: vec![
                        eterea_media::SyndicatedVariant {
                            content_type: Some("application/x-mpegURL".into()),
                            bitrate: None,
                            url: "https://video.twimg.com/x.m3u8".into(),
                        },
                        eterea_media::SyndicatedVariant {
                            content_type: Some("video/mp4".into()),
                            bitrate: Some(320000),
                            url: "https://video.twimg.com/low.mp4".into(),
                        },
                        eterea_media::SyndicatedVariant {
                            content_type: Some("video/mp4".into()),
                            bitrate: Some(1280000),
                            url: "https://video.twimg.com/high.mp4".into(),
                        },
                    ],
                },
            ],
            quoted: Some(SyndicatedQuote {
                id: "2".into(),
                text: "quoted".into(),
                created_at: Some("2020-01-01T00:00:00.000Z".into()),
                author: eterea_media::SyndicatedAuthor {
                    id: "10".into(),
                    handle: "other".into(),
                    name: "Other".into(),
                    avatar_url: None,
                },
                media: Vec::new(),
            }),
            links: vec![eterea_media::SyndicatedLink {
                url: "https://t.co/x".into(),
                expanded_url: Some("https://x.com/other/status/2".into()),
                display_url: Some("x.com/other/status/2".into()),
            }],
            hashtags: vec!["Film".into(), "#film".into(), "books".into()],
        }
    }

    #[test]
    fn payload_maps_to_the_core_model() {
        let bookmark =
            bookmark_from_payload("existing-id", "https://x.com/writer/status/1", &payload())
                .expect("maps");
        assert_eq!(bookmark.id, "existing-id");
        assert_eq!(
            bookmark.tweeted_at.to_rfc3339(),
            "2026-11-16T12:00:01+00:00"
        );
        assert_eq!(bookmark.author_handle, "writer");
        assert_eq!(bookmark.tags, vec!["books".to_string(), "film".to_string()]);
        assert_eq!(bookmark.media.len(), 2);
        assert_eq!(bookmark.media[0].media_type, MediaType::Image);
        assert_eq!(bookmark.media[0].alt_text.as_deref(), Some("alt"));
        let video = &bookmark.media[1];
        assert_eq!(video.media_type, MediaType::Video);
        assert_eq!(video.duration_ms, Some(5000));
        assert_eq!(
            video.playback_url.as_deref(),
            Some("https://video.twimg.com/high.mp4")
        );
        assert_eq!(
            video.poster_url.as_deref(),
            Some("https://pbs.twimg.com/thumb/poster.jpg")
        );
        let quote = bookmark.quoted.expect("quote");
        assert_eq!(quote.author_handle, "other");
        assert_eq!(quote.tweet_url, "https://x.com/other/status/2");
        assert_eq!(bookmark.links.len(), 1);
        assert_eq!(bookmark.links[0].kind, eterea_core::LinkKind::Tweet);
    }

    #[test]
    fn enrichment_merges_into_an_existing_row_without_history() {
        use chrono::TimeZone;
        let db = Database::open_memory().unwrap();
        let sparse = eterea_core::BookmarkBuilder::new()
            .tweet_url("https://x.com/writer/status/1")
            .content("hello")
            .tweeted_at(Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 1).unwrap())
            .author_handle("writer")
            .author_name("Old Name")
            .build()
            .unwrap();
        let existing_id = sparse.id.clone();
        db.insert_bookmarks(&[sparse]).unwrap();
        let history_before = db.history_list(0, 10, true).unwrap().1;
        db.set_read(&existing_id, true).unwrap();

        let enriched =
            bookmark_from_payload(&existing_id, "https://x.com/writer/status/1", &payload())
                .expect("maps");
        assert_eq!(db.enrich_bookmarks(&[enriched]).unwrap(), 1);

        let stored = db.get_bookmarks(0, 10).unwrap().remove(0);
        assert_eq!(stored.id, existing_id);
        assert_eq!(stored.author_name, "Writer");
        assert_eq!(stored.media.len(), 2);
        assert!(stored.quoted.is_some());
        assert_eq!(stored.tags, vec!["books".to_string(), "film".to_string()]);
        assert!(stored.is_read, "read state is untouched by repair");
        assert_eq!(
            db.history_list(0, 10, true).unwrap().1,
            history_before,
            "repair must not write a history event"
        );
    }

    #[test]
    fn missing_created_at_cannot_map() {
        let mut payload = payload();
        payload.created_at = None;
        assert!(bookmark_from_payload("id", "https://x.com/w/status/1", &payload).is_none());
    }
}
