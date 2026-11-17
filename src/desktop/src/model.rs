//! Display rows converted from the archive model, plus the synthetic deck used
//! for visual/headless checks.

use chrono::{DateTime, Datelike, Utc};
use eterea_core::{Bookmark, LinkEntity, Media, MediaType};

#[derive(Clone)]
pub struct MediaItem {
    pub key: String,
    pub alt: String,
    pub video_url: Option<String>,
    pub video: bool,
    pub width: f32,
    pub height: f32,
    pub duration: Option<i64>,
}

#[derive(Clone)]
pub struct Quote {
    #[allow(dead_code)]
    pub tweet_url: String,
    pub name: String,
    pub handle: String,
    pub date: String,
    pub text: String,
    pub avatar_key: Option<String>,
    pub thumb_key: Option<String>,
    pub unavailable: bool,
}

#[derive(Clone)]
pub struct Post {
    pub id: String,
    pub tweet_url: String,
    pub name: String,
    pub handle: String,
    pub date: String,
    pub time: String,
    pub text: String,
    pub tags: Vec<String>,
    pub links: Vec<LinkEntity>,
    pub is_read: bool,
    pub avatar_key: Option<String>,
    pub quote: Option<Quote>,
    pub media: Vec<MediaItem>,
    pub favorite: bool,
    pub media_loaded: bool,
    pub media_page: usize,
}

impl Post {
    pub fn from_bookmark(bookmark: &Bookmark) -> Self {
        let quote = bookmark.quoted.as_ref().map(quote_from_bookmark);
        Self {
            id: bookmark.id.clone(),
            tweet_url: bookmark.tweet_url.clone(),
            name: bookmark.author_name.clone(),
            handle: format!("@{}", bookmark.author_handle),
            date: display_date(bookmark.tweeted_at),
            time: bookmark.tweeted_at.format("%H:%M").to_string(),
            text: bookmark.content.clone(),
            tags: bookmark.tags.clone(),
            links: bookmark.links.clone(),
            is_read: bookmark.is_read,
            avatar_key: bookmark
                .author_profile_image
                .as_deref()
                .map(best_avatar_url),
            quote,
            media: bookmark.media.iter().filter_map(media_item).collect(),
            favorite: bookmark.is_favorite,
            media_loaded: false,
            media_page: 0,
        }
    }
}

fn quote_from_bookmark(quote: &eterea_core::QuotedTweet) -> Quote {
    Quote {
        tweet_url: quote.tweet_url.clone(),
        name: if quote.author_name.is_empty() {
            quote.author_handle.clone()
        } else {
            quote.author_name.clone()
        },
        handle: if quote.author_handle.is_empty() {
            String::new()
        } else {
            format!("@{}", quote.author_handle)
        },
        date: quote.tweeted_at.map(display_date).unwrap_or_default(),
        text: quote.content.clone(),
        avatar_key: quote.author_profile_image.as_deref().map(best_avatar_url),
        thumb_key: quote.media.iter().find_map(media_item).map(|item| item.key),
        unavailable: quote.unavailable
            || (quote.content.is_empty() && quote.author_handle.is_empty()),
    }
}

fn media_item(media: &Media) -> Option<MediaItem> {
    let key = match media.media_type {
        MediaType::Image => Some(best_media_url(&media.url)),
        MediaType::Video | MediaType::Gif => media
            .poster_url
            .as_deref()
            .or(media.preview_url.as_deref())
            .map(best_media_url),
        MediaType::Unknown => None,
    }?;
    let width = media
        .width
        .filter(|w| *w > 0)
        .map(|w| w as f32)
        .unwrap_or(720.0);
    let height = media
        .height
        .filter(|h| *h > 0)
        .map(|h| h as f32)
        .unwrap_or(480.0);
    Some(MediaItem {
        key,
        alt: media.alt_text.clone().unwrap_or_default(),
        video_url: if media.media_type == MediaType::Video {
            media.resolved_playback_url().map(str::to_string)
        } else {
            None
        },
        video: matches!(media.media_type, MediaType::Video | MediaType::Gif),
        width,
        height,
        duration: media.duration_ms,
    })
}

pub fn display_date(date: DateTime<Utc>) -> String {
    let day = date.day();
    let month = match date.month() {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    };
    format!("{day} {month} {}", date.year())
}

pub fn duration_label(ms: Option<i64>) -> String {
    let Some(ms) = ms.filter(|ms| *ms > 0) else {
        return String::new();
    };
    let seconds = ms / 1000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

// ---------------------------------------------------------------------------
// Demo deck. Keys are `demo:<asset>` and are preloaded from the test assets.
// ---------------------------------------------------------------------------

struct DemoSpec {
    id: &'static str,
    name: &'static str,
    handle: &'static str,
    date: &'static str,
    time: &'static str,
    text: &'static str,
    avatar: &'static str,
    quote: Option<(
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    )>,
    media: &'static [(&'static str, bool, f32)],
    favorite: bool,
    loaded: bool,
}

pub fn demo_posts() -> Vec<Post> {
    let mut posts = Vec::new();
    for spec in demo_specs() {
        let media = spec
            .media
            .iter()
            .map(|(name, video, aspect)| MediaItem {
                key: format!("demo:{name}"),
                alt: String::new(),
                video_url: None,
                video: *video,
                width: 720.0,
                height: 720.0 / aspect,
                duration: if *video { Some(47_000) } else { None },
            })
            .collect();
        let quote = spec
            .quote
            .map(|(name, handle, text, date, avatar, thumb)| Quote {
                tweet_url: "https://x.com/i/web/status/0".into(),
                name: name.into(),
                handle: format!("@{handle}"),
                date: date.into(),
                text: text.into(),
                avatar_key: Some(format!("demo:{avatar}")),
                thumb_key: Some(format!("demo:{thumb}")),
                unavailable: false,
            });
        posts.push(Post {
            id: spec.id.into(),
            tweet_url: "https://x.com/i/web/status/0".into(),
            name: spec.name.into(),
            handle: format!("@{}", spec.handle),
            date: spec.date.into(),
            time: spec.time.into(),
            text: spec.text.into(),
            tags: Vec::new(),
            links: Vec::new(),
            is_read: false,
            avatar_key: Some(format!("demo:{}", spec.avatar)),
            quote,
            media,
            favorite: spec.favorite,
            media_loaded: spec.loaded,
            media_page: 0,
        });
    }
    posts
}

pub fn demo_asset(key: &str) -> Option<String> {
    key.strip_prefix("demo:").map(|name| {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets/demo")
            .join(name)
            .to_string_lossy()
            .into_owned()
    })
}

fn demo_specs() -> Vec<DemoSpec> {
    vec![
        DemoSpec {
            id: "demo-noor",
            name: "Noor Haddad",
            handle: "noorhaddad",
            date: "24 Aug 2025",
            time: "14:06",
            text: "Spent the morning removing things from a reading app. Three headers, a count, a hint, a badge. None of it was the thing I came to read.",
            avatar: "av-1.jpg",
            quote: None,
            media: &[],
            favorite: false,
            loaded: true,
        },
        DemoSpec {
            id: "demo-mira",
            name: "Mira Okonkwo",
            handle: "miraokonkwo",
            date: "23 Aug 2025",
            time: "19:42",
            text: "Barbican at seven. The concrete goes pink for about ten minutes and everyone stops talking.",
            avatar: "av-2.jpg",
            quote: None,
            media: &[("barbican-1.jpg", false, 1.5), ("barbican-3.jpg", false, 1.5)],
            favorite: true,
            loaded: true,
        },
        DemoSpec {
            id: "demo-toma",
            name: "Toma Iwasaki",
            handle: "toma_i",
            date: "22 Aug 2025",
            time: "23:18",
            text: "東京の夜は雨のほうがいい。看板が全部反射するから。",
            avatar: "av-3.jpg",
            quote: None,
            media: &[("tokyo-1.jpg", false, 1.333)],
            favorite: false,
            loaded: true,
        },
        DemoSpec {
            id: "demo-elena",
            name: "Elena Voss",
            handle: "elenavoss",
            date: "22 Aug 2025",
            time: "11:20",
            text: "A reading app should feel like a table, not a feed. You put something down, you come back to it.",
            avatar: "av-4.jpg",
            quote: Some((
                "Studio Field",
                "studiofield",
                "We keep rebuilding the same page and calling it a product. The page was fine.",
                "22 Aug 2025",
                "av-9.jpg",
                "type-1.jpg",
            )),
            media: &[],
            favorite: false,
            loaded: true,
        },
        DemoSpec {
            id: "demo-samir",
            name: "Samir Patel",
            handle: "samirpatel",
            date: "21 Aug 2025",
            time: "12:03",
            text: "Three minutes inside the Pantheon at noon. The oculus does all the lighting design.",
            avatar: "av-5.jpg",
            quote: None,
            media: &[("pantheon-1.jpg", true, 1.777)],
            favorite: false,
            loaded: true,
        },
        DemoSpec {
            id: "demo-ada",
            name: "Ada Lin",
            handle: "adalin",
            date: "20 Aug 2025",
            time: "16:35",
            text: "Flea market type haul. The little 1972 specimen book has a colophon longer than most articles I read this week.",
            avatar: "av-6.jpg",
            quote: None,
            media: &[
                ("type-2.jpg", false, 1.5),
                ("type-3.jpg", false, 1.333),
                ("reading-2.jpg", false, 0.75),
            ],
            favorite: false,
            loaded: false,
        },
        DemoSpec {
            id: "demo-rui",
            name: "Rui Almeida",
            handle: "rui",
            date: "19 Aug 2025",
            time: "08:12",
            text: "A good archive remembers what you saved, not what it thinks you want.",
            avatar: "av-7.jpg",
            quote: None,
            media: &[],
            favorite: false,
            loaded: true,
        },
        DemoSpec {
            id: "demo-june",
            name: "June Park",
            handle: "junepark",
            date: "18 Aug 2025",
            time: "17:50",
            text: "Last month, in order: a garden inside a building, a room full of books, a corridor that goes nowhere, and the same corridor from below.",
            avatar: "av-8.jpg",
            quote: None,
            media: &[
                ("conservatory-2.jpg", false, 1.333),
                ("reading-1.jpg", false, 1.5),
                ("barbican-2.jpg", false, 1.5),
                ("concrete-2.jpg", false, 1.5),
            ],
            favorite: false,
            loaded: true,
        },
    ]
}

/// Ask X for the largest cached rendition of an author avatar.
pub fn best_avatar_url(url: &str) -> String {
    let base = url.trim_end_matches(['/', '\n']).to_string();
    if base.contains("pbs.twimg.com/profile_images/") {
        if let Some((path, query)) = base.split_once('?') {
            return format!("{}?{}", upgrade_avatar_path(path), query);
        }
        return upgrade_avatar_path(&base);
    }
    base
}

fn upgrade_avatar_path(path: &str) -> String {
    let mut out = path.to_string();
    for suffix in ["_normal", "_mini", "_bigger", "_reasonably_small"] {
        if out.ends_with(&format!("{suffix}.jpg")) || out.ends_with(&format!("{suffix}.png")) {
            let ext = if out.ends_with(".png") {
                ".png"
            } else {
                ".jpg"
            };
            let stem = out.trim_end_matches(&format!("{suffix}{ext}"));
            out = format!("{stem}_400x400{ext}");
            break;
        }
    }
    out
}

/// Ask X for the large rendition of a media image. External URLs are left alone.
pub fn best_media_url(url: &str) -> String {
    let trimmed = url.trim();
    if !trimmed.starts_with("https://pbs.twimg.com/") {
        return trimmed.to_string();
    }
    let (path, query) = match trimmed.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (trimmed, None),
    };
    let mut params: Vec<(String, String)> = query
        .map(|query| {
            query
                .split('&')
                .filter_map(|part| part.split_once('='))
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        })
        .unwrap_or_default();
    params.retain(|(key, _)| key != "name");
    params.push(("name".into(), "large".into()));
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatars_use_the_largest_common_rendition() {
        assert_eq!(
            best_avatar_url("https://pbs.twimg.com/profile_images/1/ada_normal.jpg"),
            "https://pbs.twimg.com/profile_images/1/ada_400x400.jpg"
        );
        assert_eq!(
            best_avatar_url("https://pbs.twimg.com/profile_images/1/ada.png"),
            "https://pbs.twimg.com/profile_images/1/ada.png"
        );
    }

    #[test]
    fn media_images_ask_for_the_large_rendition() {
        assert_eq!(
            best_media_url("https://pbs.twimg.com/media/abc?format=jpg&name=small"),
            "https://pbs.twimg.com/media/abc?format=jpg&name=large"
        );
        assert_eq!(
            best_media_url("https://pbs.twimg.com/media/abc.jpg"),
            "https://pbs.twimg.com/media/abc.jpg?name=large"
        );
        assert_eq!(
            best_media_url("https://example.com/a.jpg"),
            "https://example.com/a.jpg"
        );
    }
}
