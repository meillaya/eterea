//! Media attachment data model.

use eterea_logic::{best_mp4_playback_url, VideoVariant};
use serde::{Deserialize, Serialize};

/// Data-layer cap on attachments per bookmark. The exports can carry up to
/// four photos per post and, in some archive shapes, more; eterea keeps up to
/// twenty so a gallery never has to drop data the export carried.
pub const MAX_MEDIA_PER_BOOKMARK: usize = 20;

/// Media attachment metadata stored by eterea.
///
/// This intentionally stores only source metadata and remote URLs. It does not
/// cache, download, or persist media bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Media {
    pub url: String,
    pub media_type: MediaType,
    pub alt_text: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub source_media_key: Option<String>,
    pub source_type: Option<String>,
    pub preview_url: Option<String>,
    pub variant_url: Option<String>,
    pub variants_json: Option<String>,
    /// Video/GIF duration in milliseconds, when the export carries it.
    #[serde(default)]
    pub duration_ms: Option<i64>,
    /// Remote HTTPS poster image for a video/GIF, when the export carries one.
    #[serde(default)]
    pub poster_url: Option<String>,
    /// Best progressive MP4 playback URL chosen from the export variants.
    ///
    /// HLS playlists are never selected: the local player opens a progressive
    /// file. `None` for still images and for videos without an MP4 variant.
    #[serde(default)]
    pub playback_url: Option<String>,
}

impl Media {
    pub fn new(url: impl Into<String>, media_type: MediaType) -> Self {
        Self {
            url: url.into(),
            media_type,
            alt_text: None,
            width: None,
            height: None,
            source_media_key: None,
            source_type: None,
            preview_url: None,
            variant_url: None,
            variants_json: None,
            duration_ms: None,
            poster_url: None,
            playback_url: None,
        }
    }

    pub fn with_alt_text(mut self, alt_text: Option<String>) -> Self {
        self.alt_text = non_empty(alt_text);
        self
    }

    pub fn with_dimensions(mut self, width: Option<i64>, height: Option<i64>) -> Self {
        self.width = positive_dimension(width);
        self.height = positive_dimension(height);
        self
    }

    pub fn with_source(mut self, media_key: Option<String>, source_type: Option<String>) -> Self {
        self.source_media_key = non_empty(media_key);
        self.source_type = non_empty(source_type);
        self
    }

    pub fn with_preview_urls(
        mut self,
        preview_url: Option<String>,
        variant_url: Option<String>,
    ) -> Self {
        self.preview_url = non_empty(preview_url);
        self.variant_url = non_empty(variant_url);
        self
    }

    pub fn with_variants_json(mut self, variants_json: Option<String>) -> Self {
        self.variants_json = non_empty(variants_json);
        self
    }

    /// Attach video/GIF display metadata: duration and poster URL.
    pub fn with_display_metadata(
        mut self,
        duration_ms: Option<i64>,
        poster_url: Option<String>,
    ) -> Self {
        self.duration_ms = duration_ms.filter(|duration| *duration > 0);
        self.poster_url = non_empty(poster_url);
        self
    }

    pub fn with_duration_ms(mut self, duration_ms: Option<i64>) -> Self {
        self.duration_ms = duration_ms.filter(|duration| *duration > 0);
        self
    }

    pub fn with_poster_url(mut self, poster_url: Option<String>) -> Self {
        self.poster_url = non_empty(poster_url);
        self
    }

    /// Attach the best progressive MP4 URL.
    pub fn with_playback_url(mut self, playback_url: Option<String>) -> Self {
        self.playback_url = non_empty(playback_url);
        self
    }

    /// Playback source for the local player: the stored best MP4, or the
    /// legacy `variant_url` when it is already a progressive MP4.
    pub fn resolved_playback_url(&self) -> Option<&str> {
        self.playback_url.as_deref().or_else(|| {
            self.variant_url.as_deref().filter(|url| {
                url.to_ascii_lowercase().ends_with(".mp4")
                    && !url.to_ascii_lowercase().contains(".m3u8")
            })
        })
    }
}

/// Choose the best progressive MP4 URL from a JSON variants payload.
///
/// Used by the migration that backfills `media.playback_url` for rows written
/// before the column existed, and by the parser when it already has the raw
/// JSON. Malformed payloads yield `None`.
pub fn playback_url_from_variants_json(variants_json: Option<&str>) -> Option<String> {
    let variants_json = variants_json?;
    let value: serde_json::Value = serde_json::from_str(variants_json).ok()?;
    let variants = value.as_array()?;

    let urls: Vec<String> = variants
        .iter()
        .filter_map(|variant| {
            variant
                .get("url")
                .and_then(|url| url.as_str())
                .map(str::to_string)
        })
        .collect();
    let content_types: Vec<Option<String>> = variants
        .iter()
        .map(|variant| {
            variant
                .get("content_type")
                .and_then(|content_type| content_type.as_str())
                .map(str::to_string)
        })
        .collect();
    let bit_rates: Vec<Option<u64>> = variants
        .iter()
        .map(|variant| {
            variant
                .get("bit_rate")
                .and_then(|bit_rate| bit_rate.as_u64())
        })
        .collect();

    // Borrow the owned strings for the pure selector.
    let url_refs: Vec<&str> = urls.iter().map(String::as_str).collect();
    let content_type_refs: Vec<Option<&str>> =
        content_types.iter().map(|value| value.as_deref()).collect();
    let borrowed: Vec<VideoVariant<'_>> = (0..urls.len())
        .map(|index| VideoVariant {
            url: url_refs[index],
            content_type: content_type_refs[index],
            bit_rate: bit_rates[index],
        })
        .collect();

    best_mp4_playback_url(&borrowed).map(str::to_string)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MediaType {
    Image,
    Video,
    Gif,
    Unknown,
}

impl MediaType {
    pub fn as_storage_str(&self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Gif => "gif",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_storage_str(value: &str) -> Self {
        match value {
            "image" => Self::Image,
            "video" => Self::Video,
            "gif" => Self::Gif,
            _ => Self::Unknown,
        }
    }

    pub fn from_source_type(value: &str) -> Self {
        match value {
            "photo" | "image" => Self::Image,
            "video" => Self::Video,
            "animated_gif" | "gif" => Self::Gif,
            _ => Self::Unknown,
        }
    }

    pub fn from_url(url: &str) -> Self {
        let lower = url.to_lowercase();
        if lower.contains(".gif") || lower.contains("gif") {
            Self::Gif
        } else if lower.contains(".mp4") || lower.contains("video") {
            Self::Video
        } else if lower.contains(".jpg")
            || lower.contains(".jpeg")
            || lower.contains(".png")
            || lower.contains(".webp")
            || lower.contains("pbs.twimg.com")
        {
            Self::Image
        } else {
            Self::Unknown
        }
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn positive_dimension(value: Option<i64>) -> Option<i64> {
    value.filter(|dimension| *dimension > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_constructor_keeps_metadata_empty_by_default() {
        let media = Media::new("https://pbs.twimg.com/media/a.jpg", MediaType::Image);

        assert_eq!(media.url, "https://pbs.twimg.com/media/a.jpg");
        assert_eq!(media.media_type, MediaType::Image);
        assert_eq!(media.alt_text, None);
        assert_eq!(media.preview_url, None);
        assert_eq!(media.variants_json, None);
    }

    #[test]
    fn metadata_builders_drop_empty_text_and_invalid_dimensions() {
        let media = Media::new("https://video.twimg.com/a.mp4", MediaType::Video)
            .with_alt_text(Some("  demo clip  ".to_string()))
            .with_dimensions(Some(1280), Some(0))
            .with_source(Some(" 3_123 ".to_string()), Some("".to_string()));

        assert_eq!(media.alt_text.as_deref(), Some("demo clip"));
        assert_eq!(media.width, Some(1280));
        assert_eq!(media.height, None);
        assert_eq!(media.source_media_key.as_deref(), Some("3_123"));
        assert_eq!(media.source_type, None);
    }

    #[test]
    fn playback_url_uses_highest_bitrate_mp4_and_never_hls() {
        let variants = r#"[
            {"url":"https://video.twimg.com/a/hls.m3u8","content_type":"application/x-mpegURL","bit_rate":9999999},
            {"url":"https://video.twimg.com/a/320.mp4","content_type":"video/mp4","bit_rate":320000},
            {"url":"https://video.twimg.com/a/720.mp4","content_type":"video/mp4","bit_rate":2176000}
        ]"#;
        assert_eq!(
            playback_url_from_variants_json(Some(variants)).as_deref(),
            Some("https://video.twimg.com/a/720.mp4")
        );
        assert_eq!(playback_url_from_variants_json(Some("not json")), None);
        assert_eq!(playback_url_from_variants_json(None), None);

        let media = Media::new("https://video.twimg.com/a/720.mp4", MediaType::Video)
            .with_playback_url(playback_url_from_variants_json(Some(variants)));
        assert_eq!(
            media.resolved_playback_url(),
            Some("https://video.twimg.com/a/720.mp4")
        );

        let legacy = Media::new("https://video.twimg.com/a/clip.mp4", MediaType::Video)
            .with_preview_urls(None, Some("https://video.twimg.com/a/clip.mp4".to_string()));
        assert_eq!(
            legacy.resolved_playback_url(),
            Some("https://video.twimg.com/a/clip.mp4"),
            "legacy mp4 variant_url remains playable"
        );
    }
}
