use super::AppServices;
use crate::types::BookmarkQuery;
use chrono::{DateTime, Local, LocalResult, NaiveDate, TimeZone, Utc};
use tempfile::NamedTempFile;

fn sample_json() -> &'static str {
    include_str!("../../../../../fixtures/import/bookmarks.json")
}

fn local_boundary(date: NaiveDate, end_of_day: bool) -> DateTime<Utc> {
    let naive = if end_of_day {
        date.and_hms_opt(23, 59, 59)
    } else {
        date.and_hms_opt(0, 0, 0)
    }
    .expect("date boundary should be valid");

    let local = match Local.from_local_datetime(&naive) {
        LocalResult::Single(boundary) => boundary,
        LocalResult::Ambiguous(earliest, latest) => {
            if end_of_day {
                latest
            } else {
                earliest
            }
        }
        LocalResult::None => panic!("local timezone could not represent boundary"),
    };

    local.with_timezone(&Utc)
}

#[test]
fn imports_content_and_surfaces_stats() {
    let services = AppServices::open_memory().expect("in-memory services should open");

    let imported = services
        .import_content("sample.json", sample_json())
        .expect("json import should succeed");
    assert!(imported > 0, "expected imported bookmarks");

    let stats = services.stats().expect("stats should load");
    assert!(stats.total_bookmarks >= imported as i64);
    assert!(stats.unique_authors > 0);
}

#[test]
fn filters_bookmarks_by_query_and_tag() {
    let services = AppServices::open_memory().expect("in-memory services should open");
    services
        .import_content("sample.json", sample_json())
        .expect("json import should succeed");

    let first_page = services
        .query_bookmarks(&BookmarkQuery {
            query: Some("rust".to_string()),
            limit: 20,
            ..BookmarkQuery::default()
        })
        .expect("query should succeed");
    assert!(
        !first_page.items.is_empty(),
        "expected at least one rust result"
    );

    let tagged = services
        .query_bookmarks(&BookmarkQuery {
            tag: first_page.items[0].tags.first().cloned(),
            limit: 20,
            ..BookmarkQuery::default()
        })
        .expect("tag query should succeed");
    assert!(!tagged.items.is_empty(), "expected tagged results");
}

#[test]
fn persists_to_disk_for_restart_like_workflow() {
    let file = NamedTempFile::new().expect("temp file should exist");
    let path = file.path().to_path_buf();
    drop(file);

    {
        let services = AppServices::open(&path).expect("disk-backed services should open");
        services
            .import_content("sample.json", sample_json())
            .expect("json import should succeed");
    }

    let reopened = AppServices::open(&path).expect("reopened services should open");
    let page = reopened.list_bookmarks(0, 20).expect("page should load");
    assert!(
        !page.items.is_empty(),
        "expected persisted bookmarks after reopen"
    );
}

#[test]
fn filters_by_author_date_and_media_and_supports_delete() {
    let services = AppServices::open_memory().expect("in-memory services should open");
    services
        .import_content("sample.json", sample_json())
        .expect("json import should succeed");

    let seed_page = services
        .list_bookmarks(0, 200)
        .expect("seed page should load");
    let target = seed_page
        .items
        .iter()
        .find(|bookmark| !bookmark.media.is_empty())
        .cloned()
        .expect("expected at least one bookmark with media");

    let date = target.tweeted_at.with_timezone(&Local).date_naive();
    let from = local_boundary(date, false).to_rfc3339();
    let to = local_boundary(date, true).to_rfc3339();

    let filtered = services
        .query_bookmarks(&BookmarkQuery {
            author: Some(target.author_handle.clone()),
            from_date: Some(from),
            to_date: Some(to),
            has_media: Some(true),
            limit: 200,
            ..BookmarkQuery::default()
        })
        .expect("compound query should succeed");

    assert!(
        filtered
            .items
            .iter()
            .any(|bookmark| bookmark.id == target.id),
        "expected the target bookmark to remain visible under combined filters"
    );
    assert!(
        filtered
            .items
            .iter()
            .all(|bookmark| bookmark.author_handle == target.author_handle),
        "expected author filter to be respected"
    );
    assert!(
        filtered
            .items
            .iter()
            .all(|bookmark| !bookmark.media.is_empty()),
        "expected media filter to be respected"
    );

    let deleted = services
        .delete_bookmark(&target.id)
        .expect("delete should succeed");
    assert!(deleted, "expected the bookmark to be deleted");

    let after_delete = services
        .list_bookmarks(0, 200)
        .expect("page should load after delete");
    assert!(
        after_delete
            .items
            .iter()
            .all(|bookmark| bookmark.id != target.id),
        "expected the deleted bookmark to disappear from the library"
    );
    assert_eq!(after_delete.total, seed_page.total - 1);
}

#[test]
fn cancelling_import_before_the_write_reports_cancelled_and_writes_nothing() {
    use std::io::Write;
    use std::sync::atomic::AtomicBool;

    let services = AppServices::open_memory().expect("in-memory services should open");
    let mut file = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("temp file");
    file.write_all(sample_json().as_bytes()).unwrap();

    let cancel = AtomicBool::new(true);
    let ticks = std::cell::RefCell::new(Vec::new());
    let summary = services
        .import_files_with_progress(&[file.path().to_path_buf()], &cancel, |tick| {
            ticks.borrow_mut().push(tick)
        })
        .expect("cancelled import returns a summary, not an error");

    assert!(summary.cancelled);
    assert_eq!(summary.inserted, 0);
    assert_eq!(summary.duplicates, 0);
    assert!(summary.history_id.is_none());
    assert!(
        !ticks.borrow().is_empty(),
        "progress should be reported before the cancel"
    );
}

#[test]
fn import_progress_reports_reading_then_saving() {
    use std::io::Write;
    use std::sync::atomic::AtomicBool;

    let services = AppServices::open_memory().expect("in-memory services should open");
    let mut file = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("temp file");
    file.write_all(sample_json().as_bytes()).unwrap();

    let cancel = AtomicBool::new(false);
    let ticks = std::cell::RefCell::new(Vec::new());
    let summary = services
        .import_files_with_progress(&[file.path().to_path_buf()], &cancel, |tick| {
            ticks.borrow_mut().push(tick)
        })
        .expect("import should succeed");

    assert!(!summary.cancelled);
    assert!(summary.inserted > 0);
    let ticks = ticks.borrow();
    assert!(ticks
        .iter()
        .any(|tick| tick.phase == crate::types::ImportPhase::Reading));
    assert!(ticks
        .iter()
        .any(|tick| tick.phase == crate::types::ImportPhase::Saving));
    let last = ticks.last().expect("ticks");
    assert!(last.rows >= summary.inserted);
}

#[test]
fn preview_duplicate_count_matches_the_committed_duplicates() {
    let services = AppServices::open_memory().expect("in-memory services should open");
    let json = r#"[
        {"tweet_url":"https://x.com/a/status/1","full_text":"first copy","tweeted_at":"2024-05-01T10:00:00Z","screen_name":"a","name":"A"},
        {"tweet_url":"https://x.com/a/status/1","full_text":"duplicate inside the file","tweeted_at":"2024-05-01T10:00:00Z","screen_name":"a","name":"A"},
        {"tweet_url":"https://x.com/a/status/2","full_text":"new post","tweeted_at":"2024-05-01T11:00:00Z","screen_name":"a","name":"A"}
    ]"#;
    let preview = services
        .preview_import_content("batch.json", json)
        .expect("preview should parse");
    assert_eq!(preview.bookmark_count, 3);
    assert_eq!(
        preview.duplicate_count, 1,
        "the repeated tweet URL inside the file must be counted as a duplicate"
    );

    let summary = services
        .import_content_with_preview("batch.json", json)
        .expect("import should succeed");
    assert_eq!(summary.imported_count, 2);
    assert_eq!(
        summary.imported_count + summary.preview.duplicate_count,
        preview.bookmark_count
    );
}
