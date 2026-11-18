//! Guardrail tests for the background database actor.
//!
//! These exercise the public [`StoreHandle`] exactly like the desktop shell
//! will: queue a typed request, poll a typed response, and compare request ids.

use eterea_app::{
    BookmarkQuery, CalendarRequest, CalendarYearRequest, GraphRequest, Response, StoreHandle,
};
use std::time::Duration;
use tempfile::NamedTempFile;

const SAMPLE_JSON: &str = include_str!("../../../fixtures/import/bookmarks.json");

fn recv(handle: &StoreHandle) -> Response {
    handle
        .recv_timeout(Duration::from_secs(5))
        .expect("store actor should answer")
}

fn imported_handle() -> StoreHandle {
    let handle = StoreHandle::open_memory().expect("in-memory actor should open");
    let request_id = handle.import_content("bookmarks.json", SAMPLE_JSON);
    match recv(&handle) {
        Response::Import { id, result } => {
            assert_eq!(id, request_id);
            assert_eq!(result.expect("fixture import should succeed"), 3);
        }
        other => panic!("expected import response, got {other:?}"),
    }
    handle
}

#[test]
fn actor_covers_feed_detail_mutations_stats_and_indexes() {
    let handle = imported_handle();

    // Feed page (query path with no filters).
    let feed_id = handle.feed(BookmarkQuery {
        limit: 10,
        ..BookmarkQuery::default()
    });
    let feed = match recv(&handle) {
        Response::Feed { id, result } => {
            assert_eq!(id, feed_id);
            result.expect("feed should load")
        }
        other => panic!("expected feed response, got {other:?}"),
    };
    assert_eq!(feed.total, 3);
    assert_eq!(feed.items.len(), 3);

    // Filtered query still goes through the same typed channel.
    let filtered_id = handle.feed(BookmarkQuery {
        tag: Some("design".to_string()),
        limit: 10,
        ..BookmarkQuery::default()
    });
    let filtered = match recv(&handle) {
        Response::Feed { id, result } => {
            assert_eq!(id, filtered_id);
            result.expect("filtered feed should load")
        }
        other => panic!("expected feed response, got {other:?}"),
    };
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.items[0].author_handle, "bea_design");

    let target_id = feed.items[0].id.clone();

    // Detail.
    let detail_id = handle.detail(&target_id);
    match recv(&handle) {
        Response::Detail { id, result } => {
            assert_eq!(id, detail_id);
            let bookmark = result
                .expect("detail should load")
                .expect("bookmark exists");
            assert_eq!(bookmark.id, target_id);
        }
        other => panic!("expected detail response, got {other:?}"),
    }

    // Favorite mutation, then stats reflect it.
    let toggle_id = handle.toggle_favorite(&target_id);
    match recv(&handle) {
        Response::ToggleFavorite { id, result } => {
            assert_eq!(id, toggle_id);
            assert!(result.expect("toggle should succeed"));
        }
        other => panic!("expected toggle response, got {other:?}"),
    }
    let stats_id = handle.stats();
    match recv(&handle) {
        Response::Stats { id, result } => {
            assert_eq!(id, stats_id);
            let stats = result.expect("stats should load");
            assert_eq!(stats.total_bookmarks, 3);
            assert_eq!(stats.favorite_bookmarks, 1);
        }
        other => panic!("expected stats response, got {other:?}"),
    }

    // Directories.
    let authors_id = handle.author_index();
    match recv(&handle) {
        Response::AuthorIndex { id, result } => {
            assert_eq!(id, authors_id);
            assert_eq!(result.expect("author index should load").len(), 3);
        }
        other => panic!("expected author index response, got {other:?}"),
    }
    let topics_id = handle.topic_index();
    match recv(&handle) {
        Response::TopicIndex { id, result } => {
            assert_eq!(id, topics_id);
            assert_eq!(result.expect("topic index should load").len(), 4);
        }
        other => panic!("expected topic index response, got {other:?}"),
    }

    // Graph and calendar data.
    let graph_id = handle.graph(GraphRequest::default());
    match recv(&handle) {
        Response::Graph { id, result } => {
            assert_eq!(id, graph_id);
            let graph = result.expect("graph should load");
            assert!(!graph.nodes.is_empty());
        }
        other => panic!("expected graph response, got {other:?}"),
    }
    let calendar_id = handle.calendar(CalendarRequest::new("2026-05"));
    match recv(&handle) {
        Response::Calendar { id, result } => {
            assert_eq!(id, calendar_id);
            let calendar = result.expect("calendar should load");
            assert_eq!(calendar.weeks, 5);
            assert_eq!(calendar.days.len(), 35);
        }
        other => panic!("expected calendar response, got {other:?}"),
    }
    let year_id = handle.calendar_year(CalendarYearRequest::new(2026));
    match recv(&handle) {
        Response::CalendarYear { id, result } => {
            assert_eq!(id, year_id);
            let year = result.expect("calendar year should load");
            assert_eq!(year.year, 2026);
            assert!(!year.days.is_empty());
            assert!(year.days.iter().all(|day| day.date.starts_with("2026-")));
        }
        other => panic!("expected calendar year response, got {other:?}"),
    }

    // Delete mutation.
    let delete_id = handle.delete_bookmark(&target_id);
    match recv(&handle) {
        Response::DeleteBookmark { id, result } => {
            assert_eq!(id, delete_id);
            assert!(result.expect("delete should succeed"));
        }
        other => panic!("expected delete response, got {other:?}"),
    }
    let after_delete_id = handle.feed(BookmarkQuery {
        limit: 10,
        ..BookmarkQuery::default()
    });
    match recv(&handle) {
        Response::Feed { id, result } => {
            assert_eq!(id, after_delete_id);
            assert_eq!(result.expect("feed should reload").total, 2);
        }
        other => panic!("expected feed response, got {other:?}"),
    }

    handle.shutdown();
}

#[test]
fn actor_import_preview_is_a_dry_run_and_file_imports_work() {
    let handle = StoreHandle::open_memory().expect("in-memory actor should open");

    let preview_id = handle.import_preview_content("bookmarks.json", SAMPLE_JSON);
    match recv(&handle) {
        Response::ImportPreview { id, result } => {
            assert_eq!(id, preview_id);
            let preview = result.expect("preview should parse");
            assert_eq!(preview.bookmark_count, 3);
            assert!(!preview.sample.is_empty());
        }
        other => panic!("expected import preview response, got {other:?}"),
    }

    // A preview must not write: the feed is still empty.
    let feed_id = handle.feed(BookmarkQuery {
        limit: 10,
        ..BookmarkQuery::default()
    });
    match recv(&handle) {
        Response::Feed { id, result } => {
            assert_eq!(id, feed_id);
            assert_eq!(result.expect("feed should load").total, 0);
        }
        other => panic!("expected feed response, got {other:?}"),
    }

    // File-path import path.
    let file = tempfile::Builder::new()
        .prefix("eterea-import")
        .suffix(".json")
        .tempfile()
        .expect("temp json file should exist");
    std::fs::write(file.path(), SAMPLE_JSON).expect("fixture should write");
    let import_id = handle.import_file(file.path());
    match recv(&handle) {
        Response::Import { id, result } => {
            assert_eq!(id, import_id);
            assert_eq!(result.expect("file import should succeed"), 3);
        }
        other => panic!("expected import response, got {other:?}"),
    }

    // File-path preview sees the same format/count.
    let file_preview_id = handle.import_preview_file(file.path());
    match recv(&handle) {
        Response::ImportPreview { id, result } => {
            assert_eq!(id, file_preview_id);
            assert_eq!(result.expect("file preview should parse").bookmark_count, 3);
        }
        other => panic!("expected import preview response, got {other:?}"),
    }

    handle.shutdown();
}

#[test]
fn responses_carry_request_ids_so_stale_work_is_identifiable() {
    let handle = imported_handle();

    // Queue two requests without waiting; a single actor answers in order.
    let stale_id = handle.feed(BookmarkQuery {
        limit: 1,
        ..BookmarkQuery::default()
    });
    let newest_id = handle.stats();
    assert!(newest_id > stale_id, "request ids must be monotonic");

    let stale_response = recv(&handle);
    let newest_response = recv(&handle);
    assert_eq!(stale_response.request_id(), stale_id);
    assert_eq!(newest_response.request_id(), newest_id);
    assert_eq!(
        newest_response.request_id(),
        stale_response.request_id() + 1
    );

    // The shell compares each response id with the newest id it issued and
    // drops anything older; this is the stale-work filter, without blocking.
    let newest_issued = handle.next_request_id() - 1;
    assert!(stale_response.request_id() < newest_issued);
    assert!(newest_response.request_id() <= newest_issued);

    handle.shutdown();
}

#[test]
fn shutdown_stops_answering_and_a_reopened_actor_reads_persisted_archive() {
    let file = NamedTempFile::new().expect("temp file should exist");
    let path = file.path().to_path_buf();
    drop(file);

    {
        let handle = StoreHandle::open(&path).expect("disk actor should open");
        let import_id = handle.import_content("bookmarks.json", SAMPLE_JSON);
        match recv(&handle) {
            Response::Import { id, result } => {
                assert_eq!(id, import_id);
                assert_eq!(result.expect("disk import should succeed"), 3);
            }
            other => panic!("expected import response, got {other:?}"),
        }
        handle.shutdown();
        // After shutdown the actor thread stops; queued requests go unanswered.
        handle.stats();
        assert!(handle.recv_timeout(Duration::from_millis(200)).is_none());
    }

    let reopened = StoreHandle::open(&path).expect("reopened actor should open");
    let feed_id = reopened.feed(BookmarkQuery {
        limit: 10,
        ..BookmarkQuery::default()
    });
    match recv(&reopened) {
        Response::Feed { id, result } => {
            assert_eq!(id, feed_id);
            assert_eq!(result.expect("persisted feed should load").total, 3);
        }
        other => panic!("expected feed response, got {other:?}"),
    }
    reopened.shutdown();
}

#[test]
fn backups_round_trip_through_the_store_actor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bookmarks.db");
    let handle = StoreHandle::open(&path).unwrap();

    let import_id = handle.import_content("bookmarks.json", SAMPLE_JSON);
    match recv(&handle) {
        Response::Import { id, result } => {
            assert_eq!(id, import_id);
            let inserted = result.expect("import should succeed");
            assert_eq!(inserted, 3);
        }
        other => panic!("expected import response, got {other:?}"),
    }

    let backup_id = handle.backup_now();
    let backup_path = match recv(&handle) {
        Response::BackupNow { id, result } => {
            assert_eq!(id, backup_id);
            let backup = result.expect("backup should succeed");
            assert!(backup.bytes > 0);
            std::path::PathBuf::from(backup.path)
        }
        other => panic!("expected backup response, got {other:?}"),
    };
    assert!(backup_path.is_file());

    let list_id = handle.backup_list();
    match recv(&handle) {
        Response::BackupList { id, result } => {
            assert_eq!(id, list_id);
            let backups = result.expect("backup list should succeed");
            assert!(!backups.is_empty());
        }
        other => panic!("expected backup list response, got {other:?}"),
    }

    let check_id = handle.integrity_check();
    match recv(&handle) {
        Response::IntegrityCheck { id, result } => {
            assert_eq!(id, check_id);
            let report = result.expect("integrity check should succeed");
            assert!(report.ok, "report: {report:?}");
        }
        other => panic!("expected integrity response, got {other:?}"),
    }

    let restore_id = handle.restore_backup(&backup_path);
    match recv(&handle) {
        Response::RestoreBackup { id, result } => {
            assert_eq!(id, restore_id);
            let summary = result.expect("restore should succeed");
            assert_eq!(summary.bookmarks, 3);
        }
        other => panic!("expected restore response, got {other:?}"),
    }

    handle.shutdown();
}

#[test]
fn deletes_are_recoverable_from_trash_through_the_store_actor() {
    let handle = StoreHandle::open_memory().unwrap();
    handle.import_content("bookmarks.json", SAMPLE_JSON);
    recv(&handle);

    let feed_id = handle.feed(BookmarkQuery {
        limit: 10,
        ..BookmarkQuery::default()
    });
    let target = match recv(&handle) {
        Response::Feed { id, result } => {
            assert_eq!(id, feed_id);
            result
                .expect("feed should load")
                .items
                .first()
                .expect("archive should have a post")
                .clone()
        }
        other => panic!("expected feed response, got {other:?}"),
    };

    let delete_id = handle.delete_bookmark(&target.id);
    match recv(&handle) {
        Response::DeleteBookmark { id, result } => {
            assert_eq!(id, delete_id);
            assert!(result.expect("delete should succeed"));
        }
        other => panic!("expected delete response, got {other:?}"),
    }

    let count_id = handle.trash_count();
    match recv(&handle) {
        Response::TrashCount { id, result } => {
            assert_eq!(id, count_id);
            assert_eq!(result.expect("trash count should load"), 1);
        }
        other => panic!("expected trash count response, got {other:?}"),
    }

    let restore_id = handle.restore_deleted();
    match recv(&handle) {
        Response::RestoreDeleted { id, result } => {
            assert_eq!(id, restore_id);
            let restored = result
                .expect("restore should succeed")
                .expect("trash entry");
            assert_eq!(restored.id, target.id);
        }
        other => panic!("expected restore response, got {other:?}"),
    }

    let feed_id = handle.feed(BookmarkQuery {
        limit: 10,
        ..BookmarkQuery::default()
    });
    match recv(&handle) {
        Response::Feed { id, result } => {
            assert_eq!(id, feed_id);
            assert_eq!(result.expect("feed should reload").items.len(), 3);
        }
        other => panic!("expected feed response, got {other:?}"),
    }

    handle.shutdown();
}
