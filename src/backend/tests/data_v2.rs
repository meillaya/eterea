//! Integration coverage for the schema-v2 data layer.
//!
//! These exercise the public `eterea_core` API across import, export, undo,
//! link entities, media caps, and derived stats.

use chrono::{TimeZone, Utc};
use eterea_core::models::{
    Bookmark, BookmarkBuilder, HistoryKind, HistoryState, LinkEntity, LinkKind, Media, MediaType,
    QuotedTweet, TagMutation,
};
use eterea_core::storage::{
    BookmarkQuery, BookmarkSelection, ExportFormat, ExportRequest, ExportScope, EXPORT_CSV_MARKER,
    EXPORT_JSON_FORMAT,
};
use eterea_core::{Database, Ingester};
use std::fs;
use tempfile::TempDir;

fn media_url(tweet: &str) -> String {
    format!("https://video.twimg.com/v2/{tweet}.mp4")
}

fn rich_bookmark(tweet_id: &str, index: usize) -> Bookmark {
    let mut media = Media::new(
        format!("https://video.twimg.com/v2/{tweet_id}.mp4"),
        MediaType::Video,
    )
    .with_alt_text(Some(format!("clip {tweet_id}")))
    .with_dimensions(Some(1280), Some(720))
    .with_preview_urls(
        Some(format!("https://pbs.twimg.com/poster/{tweet_id}.jpg")),
        Some(media_url(tweet_id)),
    )
    .with_variants_json(Some(format!(
        r#"[{{"url":"https://video.twimg.com/v2/{tweet_id}/hls.m3u8","content_type":"application/x-mpegURL","bit_rate":9999999}},{{"url":"https://video.twimg.com/v2/{tweet_id}/720.mp4","content_type":"video/mp4","bit_rate":2176000}}]"#
    )))
    .with_duration_ms(Some(42_000))
    .with_poster_url(Some(format!("https://pbs.twimg.com/poster/{tweet_id}.jpg")))
    .with_playback_url(Some(format!(
        "https://video.twimg.com/v2/{tweet_id}/720.mp4"
    )));
    media.source_media_key = Some(format!("7_{index}"));

    let quote = QuotedTweet::new(
        format!("https://x.com/quoted/status/{tweet_id}"),
        format!("quoted body {tweet_id}"),
        Some(Utc.with_ymd_and_hms(2026, 4, 1, 9, 0, 0).unwrap()),
        "quoted",
        "Quoted Author",
    )
    .with_author_profile_image("https://example.com/quoted.png")
    .with_media(vec![Media::new(
        "https://pbs.twimg.com/quoted.jpg",
        MediaType::Image,
    )]);

    let mut bookmark = BookmarkBuilder::new()
        .tweet_url(format!("https://x.com/rich/status/{tweet_id}"))
        .content(format!(
            "rich bookmark {tweet_id} with UTF-8 héllo 世界 #rust"
        ))
        .note_text(format!("note {tweet_id}"))
        .tweeted_at(Utc.with_ymd_and_hms(2026, 5, 1, 12, 0, 0).unwrap())
        .author_handle("rich")
        .author_name("Rich")
        .author_profile_url("https://x.com/rich")
        .author_profile_image("https://example.com/rich.png")
        .comments("a comment")
        // Sorted insertion order matches the deterministic hydration order.
        .add_tag("design")
        .add_tag("rust")
        .add_media_metadata(media)
        .add_link(LinkEntity::new(
            "https://t.co/abc",
            Some("https://x.com/other/status/9".to_string()),
            Some("x.com/other/status/9".to_string()),
        ))
        .add_link(LinkEntity::new(
            "https://example.com/article",
            Some("https://example.com/article".to_string()),
            None,
        ))
        .quoted(quote)
        .is_read(index.is_multiple_of(2))
        .build()
        .expect("rich bookmark builds");
    bookmark.is_favorite = index == 0;
    bookmark
}

fn insert_rich(db: &Database) -> Vec<Bookmark> {
    let bookmarks = vec![rich_bookmark("5001", 0), rich_bookmark("5002", 1)];
    db.insert_bookmarks(&bookmarks).expect("rich import");
    // Return the persisted form: tags come back in stable name order, which is
    // what exports must round-trip.
    db.get_bookmarks(0, 10).expect("rich rows load")
}

fn sorted_pairs(bookmarks: &[Bookmark]) -> Vec<(String, Bookmark)> {
    let mut pairs = bookmarks
        .iter()
        .map(|bookmark| (bookmark.tweet_url.clone(), bookmark.clone()))
        .collect::<Vec<_>>();
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    pairs
}

fn export_path(dir: &TempDir, name: &str) -> std::path::PathBuf {
    dir.path().join(name)
}

#[test]
fn json_export_round_trips_every_field_through_a_fresh_database() {
    let dir = TempDir::new().unwrap();
    let source = Database::open_memory().unwrap();
    let original = insert_rich(&source);

    let path = export_path(&dir, "archive.json");
    let summary = source
        .export(&ExportRequest {
            scope: ExportScope::All,
            format: ExportFormat::Json,
            path: path.clone(),
        })
        .unwrap();
    assert_eq!(summary.count, 2);

    let raw = fs::read_to_string(&path).unwrap();
    assert!(raw.starts_with(&format!("{{\"format\":\"{EXPORT_JSON_FORMAT}\"")));
    assert!(
        !raw.contains("\"media_bytes\""),
        "no media bytes are exported"
    );

    // Re-import through the public Ingester path (auto-detected by extension).
    let parsed = Ingester::new().parse_file(&path).unwrap();
    assert_eq!(parsed.len(), 2);

    let target = Database::open_memory().unwrap();
    assert_eq!(target.insert_bookmarks(&parsed).unwrap(), 2);

    let imported = target.get_bookmarks(0, 10).unwrap();
    let expected = sorted_pairs(&original);
    let actual = sorted_pairs(&imported);

    for ((expected_url, expected), (actual_url, actual)) in expected.iter().zip(actual.iter()) {
        assert_eq!(expected_url, actual_url);
        assert_eq!(actual.id, expected.id, "ids round-trip");
        assert_eq!(actual.content, expected.content);
        assert_eq!(actual.note_text, expected.note_text);
        assert_eq!(actual.comments, expected.comments);
        assert_eq!(actual.tweeted_at, expected.tweeted_at);
        assert_eq!(actual.author_profile_url, expected.author_profile_url);
        assert_eq!(actual.author_profile_image, expected.author_profile_image);
        assert_eq!(actual.tags, expected.tags);
        assert_eq!(actual.is_read, expected.is_read);
        assert_eq!(actual.is_favorite, expected.is_favorite);
        assert_eq!(actual.links, expected.links);
        assert_eq!(actual.quoted, expected.quoted);

        assert_eq!(actual.media.len(), expected.media.len());
        for (actual_media, expected_media) in actual.media.iter().zip(expected.media.iter()) {
            assert_eq!(actual_media.url, expected_media.url);
            assert_eq!(actual_media.media_type, expected_media.media_type);
            assert_eq!(actual_media.alt_text, expected_media.alt_text);
            assert_eq!(actual_media.width, expected_media.width);
            assert_eq!(actual_media.height, expected_media.height);
            assert_eq!(actual_media.preview_url, expected_media.preview_url);
            assert_eq!(actual_media.poster_url, expected_media.poster_url);
            assert_eq!(actual_media.duration_ms, expected_media.duration_ms);
            assert_eq!(actual_media.playback_url, expected_media.playback_url);
        }
    }
}

#[test]
fn csv_export_round_trips_every_field_through_a_fresh_database() {
    let dir = TempDir::new().unwrap();
    let source = Database::open_memory().unwrap();
    let original = insert_rich(&source);

    let path = export_path(&dir, "archive.csv");
    source
        .export(&ExportRequest {
            scope: ExportScope::All,
            format: ExportFormat::Csv,
            path: path.clone(),
        })
        .unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert!(raw.starts_with(EXPORT_CSV_MARKER), "exact app spelling");
    assert!(raw.contains("héllo 世界"), "UTF-8 survives CSV escaping");

    let parsed = Ingester::new().parse_file(&path).unwrap();
    let target = Database::open_memory().unwrap();
    target.insert_bookmarks(&parsed).unwrap();

    let expected = sorted_pairs(&original);
    let actual = sorted_pairs(&target.get_bookmarks(0, 10).unwrap());
    for ((_, expected), (_, actual)) in expected.iter().zip(actual.iter()) {
        assert_eq!(actual.id, expected.id);
        assert_eq!(actual.tweet_url, expected.tweet_url);
        assert_eq!(actual.content, expected.content);
        assert_eq!(actual.tags, expected.tags);
        assert_eq!(actual.links, expected.links);
        assert_eq!(actual.is_read, expected.is_read);
        assert_eq!(actual.is_favorite, expected.is_favorite);
        assert_eq!(actual.quoted, expected.quoted);
        assert_eq!(actual.media.len(), expected.media.len());
        assert_eq!(actual.media[0].playback_url, expected.media[0].playback_url);
        assert_eq!(actual.media[0].poster_url, expected.media[0].poster_url);
        assert_eq!(actual.media[0].duration_ms, expected.media[0].duration_ms);
    }
}

#[test]
fn filtered_and_selected_exports_cover_exactly_the_selection() {
    let dir = TempDir::new().unwrap();
    let db = Database::open_memory().unwrap();
    let bookmarks = insert_rich(&db);

    let filtered_path = export_path(&dir, "filtered.json");
    let filtered = db
        .export(&ExportRequest {
            scope: ExportScope::Query(BookmarkQuery {
                unread_only: true,
                ..BookmarkQuery::default()
            }),
            format: ExportFormat::Json,
            path: filtered_path.clone(),
        })
        .unwrap();
    assert_eq!(filtered.count, 1);
    let parsed = Ingester::new().parse_file(&filtered_path).unwrap();
    assert_eq!(parsed.len(), 1);
    assert!(!parsed[0].is_read);

    let selected_path = export_path(&dir, "selected.csv");
    let selected = db
        .export(&ExportRequest {
            scope: ExportScope::Ids(vec![bookmarks[0].id.clone()]),
            format: ExportFormat::Csv,
            path: selected_path.clone(),
        })
        .unwrap();
    assert_eq!(selected.count, 1);
    let parsed = Ingester::new().parse_file(&selected_path).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, bookmarks[0].id);
}

const V2_JSON: &str = r#"{
  "data": [
    {
      "id": "8001",
      "text": "api v2 links #rust",
      "created_at": "2026-05-04T10:00:00.000Z",
      "author_id": "u1",
      "entities": {
        "urls": [
          { "url": "https://t.co/one", "expanded_url": "https://x.com/quoted/status/7001", "display_url": "x.com/quoted/status/7001", "start": 0, "end": 10 },
          { "url": "https://t.co/two", "expanded_url": "https://example.com/deep-dive", "display_url": "example.com/deep-dive", "start": 11, "end": 20 }
        ]
      }
    }
  ],
  "includes": { "users": [{ "id": "u1", "name": "Ada", "username": "ada_dev" }] }
}"#;

#[test]
fn x_api_v2_url_entities_are_classified_deduped_and_sorted() {
    let db = Database::open_memory().unwrap();
    let parsed = Ingester::new().parse_content("json", V2_JSON).unwrap();
    assert_eq!(parsed.len(), 1);
    let links = parsed[0].links.clone();
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].url, "https://t.co/one");
    assert_eq!(links[0].kind, LinkKind::Tweet);
    assert_eq!(links[1].url, "https://t.co/two");
    assert_eq!(links[1].kind, LinkKind::External);
    assert_eq!(
        links[1].display_url.as_deref(),
        Some("example.com/deep-dive"),
        "display_url is stored for rendering"
    );

    db.insert_bookmarks(&parsed).unwrap();
    let loaded = db.get_bookmarks(0, 10).unwrap().remove(0);
    assert_eq!(loaded.links, links, "links persist through storage");
    assert_eq!(
        db.search("deep-dive", 10).unwrap().len(),
        1,
        "expanded link targets are part of the search index (schema v5)"
    );
}

#[test]
fn archive_url_entity_forms_are_captured_with_quote() {
    let archive = r#"[
      {
        "tweet_url": "https://x.com/archive/status/8101",
        "full_text": "archive row with links",
        "tweeted_at": "2026-05-04T10:00:00Z",
        "screen_name": "archive",
        "name": "Archive",
        "entities": {
          "urls": [
            { "url": "https://t.co/pic", "expanded_url": "https://pic.twitter.com/pic", "display_url": "pic.twitter.com/pic" }
          ]
        },
        "links": [
          { "url": "https://example.com/a", "expanded_url": "https://example.com/a" }
        ],
        "quoted_status_id_str": "8201"
      }
    ]"#;
    let parsed = Ingester::new().parse_content("json", archive).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].links.len(), 2);
    // Links are sorted by URL: example.com precedes the t.co short link.
    assert_eq!(parsed[0].links[0].kind, LinkKind::External);
    assert_eq!(parsed[0].links[1].kind, LinkKind::Media);
    let quote = parsed[0].quoted.as_ref().expect("unavailable quote kept");
    assert!(quote.unavailable);
    assert_eq!(quote.tweet_url, "https://x.com/i/web/status/8201");
}

#[test]
fn twenty_media_are_kept_in_data_not_dropped_at_four() {
    let mut bookmark = BookmarkBuilder::new()
        .tweet_url("https://x.com/media/status/9001")
        .content("twenty attachments")
        .tweeted_at(Utc.with_ymd_and_hms(2026, 5, 4, 10, 0, 0).unwrap())
        .author_handle("media")
        .author_name("Media");
    for index in 0..20 {
        bookmark = bookmark.add_media_metadata(Media::new(
            format!("https://pbs.twimg.com/media/{index}.jpg"),
            MediaType::Image,
        ));
    }
    let bookmark = bookmark.build().unwrap();
    assert_eq!(bookmark.media.len(), 20);

    let db = Database::open_memory().unwrap();
    db.insert_bookmarks(std::slice::from_ref(&bookmark))
        .unwrap();
    let loaded = db.get_bookmark(&bookmark.id).unwrap().unwrap();
    assert_eq!(loaded.media.len(), 20);
    assert!(!loaded.media.is_empty());
}

#[test]
fn undo_delete_restores_exact_snapshots() {
    let db = Database::open_memory().unwrap();
    let first = rich_bookmark("6001", 0);
    let second = rich_bookmark("6002", 1);
    db.insert_bookmarks(&[first.clone(), second.clone()])
        .unwrap();

    let deleted = db
        .bulk_delete(&BookmarkSelection::Ids(vec![
            first.id.clone(),
            second.id.clone(),
        ]))
        .unwrap();
    assert_eq!(deleted.affected, 2);
    assert_eq!(db.count_bookmarks().unwrap(), 0);

    let undo = db.undo_latest().unwrap();
    assert_eq!(undo.kind, Some(HistoryKind::Delete));
    assert_eq!(undo.affected, 2);
    assert_eq!(undo.skipped, 0);

    let restored = db.get_bookmark(&first.id).unwrap().unwrap();
    assert_eq!(restored.tweet_url, first.tweet_url);
    assert_eq!(restored.tags, first.tags);
    assert_eq!(restored.quoted, first.quoted);
    assert_eq!(restored.links, first.links);
    assert_eq!(restored.media.len(), first.media.len());
    assert_eq!(restored.media[0].playback_url, first.media[0].playback_url);
    assert_eq!(restored.media[0].poster_url, first.media[0].poster_url);
    assert!(restored.is_read, "read state is restored exactly");

    let favorite_source = if first.is_favorite { &first } else { &second };
    let restored_favorite = db.get_bookmark(&favorite_source.id).unwrap().unwrap();
    assert!(
        restored_favorite.is_favorite,
        "favorite state is restored exactly"
    );
    let non_favorite_source = if first.is_favorite { &second } else { &first };
    assert!(
        !db.get_bookmark(&non_favorite_source.id)
            .unwrap()
            .unwrap()
            .is_favorite
    );
    assert_eq!(db.count_bookmarks().unwrap(), 2);
}

#[test]
fn only_the_newest_non_undone_event_is_undoable() {
    let db = Database::open_memory().unwrap();
    let bookmarks = insert_rich(&db);

    // Newest event: a tag edit; the import event is older.
    db.set_bookmark_tags(&bookmarks[0].id, &["systems".to_string()])
        .unwrap();
    let (events, _) = db.history_list(0, 10, true).unwrap();
    assert_eq!(events[0].kind, HistoryKind::Tag);
    assert!(events[0].can_undo);
    assert!(
        !events[1].can_undo,
        "the older import event is not undoable"
    );

    // Undo the tag edit, then the import event becomes newest non-undone.
    let tag_undo = db.undo_latest().unwrap();
    assert_eq!(tag_undo.affected, 1);
    let (events, _) = db.history_list(0, 10, true).unwrap();
    assert!(!events[0].can_undo, "the undone event is marked");
    assert!(events[1].can_undo);
    assert!(events[0].undone_at.is_some());

    let import_undo = db.undo_latest().unwrap();
    assert_eq!(import_undo.kind, Some(HistoryKind::Import));
    assert_eq!(import_undo.affected, 2);
    assert_eq!(db.count_bookmarks().unwrap(), 0);
    assert_eq!(db.undo_latest().unwrap().kind, None);
}

#[test]
fn history_detail_carries_exact_before_after_for_tag_events() {
    let db = Database::open_memory().unwrap();
    let bookmarks = insert_rich(&db);
    db.apply_tag_mutation(
        &BookmarkSelection::Ids(vec![bookmarks[0].id.clone(), bookmarks[1].id.clone()]),
        &TagMutation::Remove(vec!["design".to_string()]),
    )
    .unwrap();

    let (events, _) = db.history_list(0, 10, true).unwrap();
    let detail = db.history_detail(events[0].id).unwrap().unwrap();
    assert_eq!(detail.items.len(), 2);
    for item in &detail.items {
        assert_eq!(
            item.before,
            Some(HistoryState::Tags(vec![
                "design".to_string(),
                "rust".to_string()
            ]))
        );
        assert_eq!(
            item.after,
            Some(HistoryState::Tags(vec!["rust".to_string()]))
        );
    }

    // Undo restores the exact before tags.
    db.undo_latest().unwrap();
    for bookmark in &bookmarks {
        let tags = db.load_bookmark_tags(&bookmark.id).unwrap();
        assert_eq!(tags, bookmark.tags);
    }
}

#[test]
fn multi_file_batch_import_uses_one_transaction_and_one_history_event() {
    let dir = TempDir::new().unwrap();
    let first = dir.path().join("one.json");
    let second = dir.path().join("two.json");
    fs::write(
        &first,
        r#"[{"tweet_url":"https://x.com/batch/status/1","full_text":"batch one","tweeted_at":"2026-05-01T10:00:00Z","screen_name":"batch","name":"Batch"}]"#,
    )
    .unwrap();
    fs::write(
        &second,
        r#"[{"tweet_url":"https://x.com/batch/status/2","full_text":"batch two","tweeted_at":"2026-05-02T10:00:00Z","screen_name":"batch","name":"Batch"},{"tweet_url":"https://x.com/batch/status/1","full_text":"batch one duplicate","tweeted_at":"2026-05-01T10:00:00Z","screen_name":"batch","name":"Batch"}]"#,
    )
    .unwrap();

    let ingester = Ingester::new();
    let first_parsed = ingester.parse_file(&first).unwrap();
    let second_parsed = ingester.parse_file(&second).unwrap();

    let db = Database::open_memory().unwrap();
    let outcome = db
        .insert_bookmarks_batch(&[
            ("one.json".to_string(), first_parsed),
            ("two.json".to_string(), second_parsed),
        ])
        .unwrap();
    assert_eq!(outcome.inserted, 2);
    assert_eq!(outcome.duplicates, 1);
    assert_eq!(outcome.files.len(), 2);
    assert_eq!(outcome.files[1].duplicates, 1);
    let history_id = outcome.history_id.expect("one import event");
    let (events, total) = db.history_list(0, 10, true).unwrap();
    assert_eq!(total, 1, "one history blob for the batch");
    assert_eq!(events[0].id, history_id);
    assert_eq!(events[0].summary.files.len(), 2);

    // Duplicate counts stay honest against a re-import.
    let repeat = db
        .insert_bookmarks_batch(&[("one.json".to_string(), db.get_bookmarks(0, 10).unwrap())])
        .unwrap();
    assert_eq!(repeat.inserted, 0);
    assert_eq!(repeat.duplicates, 2);
    assert!(repeat.history_id.is_none(), "no-op import does not journal");
}

#[test]
fn favorites_and_read_counts_stay_consistent_after_bulk_mutations() {
    let db = Database::open_memory().unwrap();
    insert_rich(&db);

    let stats = db.get_stats().unwrap();
    assert_eq!(stats.total_bookmarks, 2);
    assert_eq!(stats.favorite_bookmarks, 1);
    assert_eq!(stats.unique_authors, 1);

    db.bulk_set_read(
        &BookmarkSelection::Query(BookmarkQuery {
            tag: Some("design".to_string()),
            ..BookmarkQuery::default()
        }),
        true,
    )
    .unwrap();
    let unread = db
        .search_query_page(&BookmarkQuery {
            unread_only: true,
            limit: 10,
            ..BookmarkQuery::default()
        })
        .unwrap();
    assert_eq!(unread.1, 0, "both rich fixtures carry the design tag");

    let deleted = db
        .bulk_delete(&BookmarkSelection::Query(BookmarkQuery {
            favorites_only: true,
            ..BookmarkQuery::default()
        }))
        .unwrap();
    assert_eq!(deleted.affected, 1);
    let stats = db.get_stats().unwrap();
    assert_eq!(stats.favorite_bookmarks, 0);
    assert_eq!(stats.total_bookmarks, 1);
    assert_eq!(stats.unique_authors, 1);
}

#[test]
fn tag_colors_are_stored_validated_and_returned() {
    let db = Database::open_memory().unwrap();
    let color = db.set_tag_color("rust", "#B4452F").unwrap();
    assert_eq!(color.tag, "rust");
    assert!(db.set_tag_color("rust", "nope").is_err());
    assert_eq!(db.tag_colors().unwrap().len(), 1);
    assert_eq!(db.get_tag_color("RUST").unwrap().unwrap().color, "#B4452F");
}

#[test]
fn delete_removes_the_bookmark_from_search() {
    let db = Database::open_memory().unwrap();
    let bookmark = rich_bookmark("7001", 0);
    let id = bookmark.id.clone();
    db.insert_bookmarks(&[bookmark]).unwrap();
    assert_eq!(db.search("rich bookmark", 10).unwrap().len(), 1);
    assert!(db.delete_bookmark(&id).unwrap());
    assert!(db.search("rich bookmark", 10).unwrap().is_empty());
    assert!(db.search("héllo", 10).unwrap().is_empty());
}

#[test]
fn millisecond_epoch_imports_with_the_true_date_and_keeps_calendar_working() {
    // B-CD-3 regression: a 13-digit Date.getTime() epoch used to be stored as
    // year 56302, which sorted to the top of the feed and made the calendar
    // query return a NULL day (a route error). It must import as the true
    // instant and the calendar day query must stay usable.
    let db = Database::open_memory().unwrap();
    let parsed = Ingester::new()
        .parse_content(
            "json",
            r#"[{"tweet_url":"https://x.com/ms/status/1","timestamp":1714564800000,"full_text":"millisecond epoch post","screen_name":"ms","name":"Ms"}]"#,
        )
        .unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(
        parsed[0].tweeted_at,
        Utc.with_ymd_and_hms(2024, 5, 1, 12, 0, 0).unwrap()
    );
    db.insert_bookmarks(&parsed).unwrap();

    let loaded = db.get_bookmarks(0, 10).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].tweeted_at, parsed[0].tweeted_at);
    assert_eq!(
        loaded[0].tweeted_at.format("%Y-%m-%d").to_string(),
        "2024-05-01"
    );

    let entries = db.get_bookmark_day_entries(0, i64::MAX).unwrap();
    assert_eq!(
        entries,
        vec![("2024-05-01".to_string(), loaded[0].id.clone())]
    );
}

#[test]
fn x_api_v2_conversation_id_is_captured_for_threads() {
    let json = r#"{
      "data": [{
        "id": "9101",
        "text": "thread root",
        "created_at": "2026-05-04T10:00:00.000Z",
        "author_id": "u1",
        "conversation_id": "9000",
        "entities": {}
      }],
      "includes": { "users": [{ "id": "u1", "name": "Ada", "username": "ada_dev" }] }
    }"#;
    let parsed = Ingester::new().parse_content("json", json).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].conversation_id.as_deref(), Some("9000"));
    let db = Database::open_memory().unwrap();
    db.insert_bookmarks(&parsed).unwrap();
    let loaded = db.get_bookmark(&parsed[0].id).unwrap().unwrap();
    assert_eq!(loaded.conversation_id.as_deref(), Some("9000"));
    assert_eq!(db.thread_for_bookmark(&parsed[0].id).unwrap().len(), 1);
}
