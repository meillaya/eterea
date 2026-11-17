//! Service and store-actor coverage for the schema-v2 data APIs.
//!
//! Mirrors how the Rune shell uses `AppServices`/`StoreHandle`: batch preview
//! and import, bulk selection mutations, history list/detail/undo, paged
//! directories, tag colors, CSV/JSON export round-trips, and typed responses
//! with request ids.

use eterea_app::{
    AppServices, BookmarkQuery, BookmarkSelection, DirectoryPageRequest, ExportFormat,
    ExportRequest, ExportScope, HistoryKind, HistoryListRequest, Response, StoreHandle,
    TagMutation,
};
use std::fs;
use std::time::Duration;
use tempfile::TempDir;

const SAMPLE_JSON: &str = include_str!("../../../fixtures/import/bookmarks.json");

fn imported_services() -> AppServices {
    let services = AppServices::open_memory().expect("in-memory services");
    let imported = services
        .import_content("bookmarks.json", SAMPLE_JSON)
        .expect("fixture import");
    assert_eq!(imported, 3);
    services
}

fn write(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, content).expect("fixture file writes");
    path
}

const SECOND_JSON: &str = r#"[
  {"tweet_url":"https://x.com/batch/status/2","full_text":"second batch file #ops","tweeted_at":"2026-05-06T10:00:00Z","screen_name":"batch_ops","name":"Batch Ops"},
  {"tweet_url":"https://x.com/ada_dev/status/1001","full_text":"duplicate of fixture","tweeted_at":"2026-05-01T10:15:00Z","screen_name":"ada_dev","name":"Ada Developer"}
]"#;

#[test]
fn batch_preview_is_a_dry_run_and_import_files_is_one_transaction() {
    let dir = TempDir::new().unwrap();
    let first = write(&dir, "first.json", SAMPLE_JSON);
    let second = write(&dir, "second.json", SECOND_JSON);
    let services = imported_services();

    let preview = services
        .preview_import_files(&[first.clone(), second.clone()])
        .expect("multi-file preview");
    assert_eq!(preview.files.len(), 2);
    assert_eq!(preview.total_parsed, 5);
    // 3 fixture rows + 1 overlapping row are duplicates of the existing archive.
    assert_eq!(preview.total_duplicates, 4);
    assert_eq!(preview.files[0].duplicate_count, 3);
    assert_eq!(preview.files[1].duplicate_count, 1);
    assert_eq!(
        services.list_bookmarks(0, 10).unwrap().total,
        3,
        "preview must not mutate the archive"
    );

    let summary = services
        .import_files(&[first, second])
        .expect("multi-file import");
    assert_eq!(summary.inserted, 1);
    assert_eq!(summary.duplicates, 4);
    assert_eq!(summary.files.len(), 2);
    assert_eq!(summary.files[0].inserted, 0);
    assert_eq!(summary.files[1].inserted, 1);
    assert!(summary.history_id.is_some());
    assert_eq!(services.list_bookmarks(0, 10).unwrap().total, 4);

    let history = services
        .history_list(&HistoryListRequest::default())
        .expect("history list");
    // One event for the earlier fixture import and one for the batch.
    assert_eq!(history.total, 2);
    assert_eq!(history.items[0].kind, HistoryKind::Import);
    assert_eq!(history.items[0].summary.affected, 1);
}

#[test]
fn bulk_tag_read_and_delete_flow_through_the_service_and_journal() {
    let services = imported_services();

    let tagged = services
        .bulk_tag(
            &BookmarkSelection::Query(BookmarkQuery {
                tag: Some("design".to_string()),
                ..BookmarkQuery::default()
            }),
            &TagMutation::Add(vec!["editorial".to_string()]),
        )
        .expect("bulk tag");
    assert_eq!(tagged.affected, 1);
    let history_id = tagged.history_id.expect("bulk tag journals");

    let design_page = services
        .query_bookmarks(&BookmarkQuery {
            tag: Some("editorial".to_string()),
            limit: 10,
            ..BookmarkQuery::default()
        })
        .expect("tagged page");
    assert_eq!(design_page.total, 1);

    let detail = services
        .history_detail(history_id)
        .expect("history detail")
        .expect("event exists");
    assert_eq!(detail.items.len(), 1);
    assert_eq!(
        detail.items[0].before,
        Some(eterea_app::HistoryState::Tags(vec!["design".to_string()]))
    );

    let read = services
        .bulk_set_read(
            &BookmarkSelection::Ids(vec![design_page.items[0].id.clone()]),
            true,
        )
        .expect("mark read");
    assert_eq!(read.affected, 1);
    let unread = services
        .query_bookmarks(&BookmarkQuery {
            unread_only: true,
            limit: 10,
            ..BookmarkQuery::default()
        })
        .expect("unread page");
    assert_eq!(unread.total, 2);

    let deleted = services
        .bulk_delete(&BookmarkSelection::Query(BookmarkQuery {
            tag: Some("editorial".to_string()),
            ..BookmarkQuery::default()
        }))
        .expect("bulk delete");
    assert_eq!(deleted.affected, 1);
    assert_eq!(services.list_bookmarks(0, 10).unwrap().total, 2);

    // Undo the delete, then the tag edit: newest non-undone only.
    let undo_delete = services.undo_latest().expect("undo delete");
    assert_eq!(undo_delete.kind, Some(HistoryKind::Delete));
    assert_eq!(undo_delete.affected, 1);
    assert_eq!(services.list_bookmarks(0, 10).unwrap().total, 3);

    let undo_tag = services.undo_latest().expect("undo tag");
    assert_eq!(undo_tag.kind, Some(HistoryKind::Tag));
    assert_eq!(undo_tag.affected, 1);
    let editorial = services
        .query_bookmarks(&BookmarkQuery {
            tag: Some("editorial".to_string()),
            limit: 10,
            ..BookmarkQuery::default()
        })
        .expect("tag rolled back");
    assert_eq!(editorial.total, 0);
}

#[test]
fn paged_directories_filter_and_report_totals() {
    let services = imported_services();

    let authors = services
        .author_page(&DirectoryPageRequest {
            offset: 0,
            limit: 2,
            filter: None,
        })
        .expect("author page");
    assert_eq!(authors.total, 3);
    assert_eq!(authors.items.len(), 2);
    assert!(authors.has_more);

    let filtered = services
        .author_page(&DirectoryPageRequest {
            offset: 0,
            limit: 10,
            filter: Some("bea".to_string()),
        })
        .expect("filtered author page");
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.items[0].handle, "bea_design");

    let topics = services
        .topic_page(&DirectoryPageRequest {
            offset: 0,
            limit: 10,
            filter: Some("arch".to_string()),
        })
        .expect("filtered topic page");
    assert_eq!(topics.total, 1);
    assert_eq!(topics.items[0].tag, "architecture");
}

#[test]
fn export_then_import_round_trips_through_the_service() {
    let dir = TempDir::new().unwrap();
    let services = imported_services();

    let csv_path = dir.path().join("export.csv");
    let json_path = dir.path().join("export.json");

    let csv_summary = services
        .export(&ExportRequest {
            scope: ExportScope::Query(BookmarkQuery {
                has_media: Some(true),
                ..BookmarkQuery::default()
            }),
            format: ExportFormat::Csv,
            path: csv_path.clone(),
        })
        .expect("CSV export");
    assert_eq!(csv_summary.count, 2);
    assert_eq!(csv_summary.format, ExportFormat::Csv);

    let json_summary = services
        .export(&ExportRequest {
            scope: ExportScope::All,
            format: ExportFormat::Json,
            path: json_path.clone(),
        })
        .expect("JSON export");
    assert_eq!(json_summary.count, 3);

    let bare = AppServices::open_memory().unwrap();
    assert_eq!(bare.import_file(&csv_path).unwrap(), 2);
    assert_eq!(
        bare.import_file(&json_path).unwrap(),
        1,
        "2 duplicates skipped"
    );
    assert_eq!(bare.list_bookmarks(0, 10).unwrap().total, 3);
    let loaded = bare.list_bookmarks(0, 10).unwrap();
    assert!(loaded
        .items
        .iter()
        .any(|bookmark| bookmark.author_handle == "bea_design"));
}

#[test]
fn tag_colors_are_exposed_through_the_service() {
    let services = imported_services();
    let color = services.set_tag_color("rust", "#2447B8").unwrap();
    assert_eq!(color.tag, "rust");
    assert_eq!(services.tag_colors().unwrap().len(), 1);
    assert!(services.set_tag_color("rust", "#xyzxyz").is_err());
    assert!(services.clear_tag_color("rust").unwrap());
}

fn recv(handle: &StoreHandle) -> Response {
    handle
        .recv_timeout(Duration::from_secs(10))
        .expect("store actor should answer")
}

fn actor() -> StoreHandle {
    let handle = StoreHandle::open_memory().expect("actor opens");
    let id = handle.import_content("bookmarks.json", SAMPLE_JSON);
    match recv(&handle) {
        Response::Import {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(result.expect("fixture import"), 3);
        }
        other => panic!("expected import response, got {other:?}"),
    }
    handle
}

#[test]
fn actor_answers_every_v2_command_with_typed_ids() {
    let dir = TempDir::new().unwrap();
    let handle = actor();
    handle.feed(BookmarkQuery {
        limit: 10,
        ..BookmarkQuery::default()
    });
    let feed = match recv(&handle) {
        Response::Feed { result, .. } => result.expect("feed"),
        other => panic!("expected feed, got {other:?}"),
    };
    let target = feed.items[0].clone();

    // SetTags
    let id = handle.set_tags(target.id.clone(), vec!["starred".to_string()]);
    match recv(&handle) {
        Response::SetTags {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            let bulk = result.expect("set tags");
            assert_eq!(bulk.affected, 1);
            assert!(bulk.history_id.is_some());
        }
        other => panic!("expected set tags, got {other:?}"),
    }

    // BulkTag over a query.
    let id = handle.bulk_tag(
        BookmarkSelection::Query(BookmarkQuery {
            tag: Some("starred".to_string()),
            ..BookmarkQuery::default()
        }),
        TagMutation::Add(vec!["rust".to_string()]),
    );
    match recv(&handle) {
        Response::BulkTag {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(result.expect("bulk tag").affected, 1);
        }
        other => panic!("expected bulk tag, got {other:?}"),
    }

    // BulkSetRead + read filter.
    let id = handle.bulk_set_read(
        BookmarkSelection::Query(BookmarkQuery {
            unread_only: true,
            ..BookmarkQuery::default()
        }),
        true,
    );
    match recv(&handle) {
        Response::BulkSetRead {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(result.expect("bulk set read").affected, 3);
        }
        other => panic!("expected bulk set read, got {other:?}"),
    }
    let id = handle.set_read(target.id.clone(), false);
    match recv(&handle) {
        Response::SetRead {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(result.expect("set read").affected, 1);
        }
        other => panic!("expected set read, got {other:?}"),
    }

    // Paged directories.
    let id = handle.author_page(DirectoryPageRequest {
        offset: 0,
        limit: 2,
        filter: None,
    });
    match recv(&handle) {
        Response::AuthorPage {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            let page = result.expect("author page");
            assert_eq!(page.total, 3);
            assert_eq!(page.items.len(), 2);
        }
        other => panic!("expected author page, got {other:?}"),
    }
    let id = handle.topic_page(DirectoryPageRequest {
        offset: 0,
        limit: 10,
        filter: Some("rust".to_string()),
    });
    match recv(&handle) {
        Response::TopicPage {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(result.expect("topic page").total, 1);
        }
        other => panic!("expected topic page, got {other:?}"),
    }

    // History list + detail + undo.
    let id = handle.history_list(HistoryListRequest::default());
    let history = match recv(&handle) {
        Response::HistoryList {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            result.expect("history list")
        }
        other => panic!("expected history list, got {other:?}"),
    };
    assert!(history.total >= 3);
    let event_id = history.items[0].id;
    let id = handle.history_detail(event_id);
    match recv(&handle) {
        Response::HistoryDetail {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            let detail = result.expect("history detail").expect("event exists");
            assert_eq!(detail.event.id, event_id);
        }
        other => panic!("expected history detail, got {other:?}"),
    }
    let id = handle.undo_latest();
    match recv(&handle) {
        Response::UndoLatest {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            let undo = result.expect("undo latest");
            assert_eq!(undo.kind, Some(HistoryKind::Tag));
            assert_eq!(undo.affected, 1);
        }
        other => panic!("expected undo latest, got {other:?}"),
    }

    // Tag colors.
    let id = handle.set_tag_color("rust", "#2447B8");
    match recv(&handle) {
        Response::SetTagColor {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(result.expect("set tag color").color, "#2447B8");
        }
        other => panic!("expected set tag color, got {other:?}"),
    }
    let id = handle.tag_colors();
    match recv(&handle) {
        Response::TagColors {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(result.expect("tag colors").len(), 1);
        }
        other => panic!("expected tag colors, got {other:?}"),
    }

    // Batch import preview/import and export.
    let path = write(&dir, "second.json", SECOND_JSON);
    let id = handle.preview_files(vec![path.clone()]);
    match recv(&handle) {
        Response::PreviewFiles {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            let preview = result.expect("batch preview");
            assert_eq!(preview.files.len(), 1);
            assert_eq!(preview.total_parsed, 2);
        }
        other => panic!("expected preview files, got {other:?}"),
    }
    let id = handle.import_files(vec![path]);
    match recv(&handle) {
        Response::ImportFiles {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            let summary = result.expect("batch import");
            assert_eq!(summary.inserted, 1);
            assert_eq!(summary.duplicates, 1);
            assert!(summary.history_id.is_some());
        }
        other => panic!("expected import files, got {other:?}"),
    }

    let export_path = dir.path().join("actor-export.json");
    let id = handle.export(ExportRequest {
        scope: ExportScope::All,
        format: ExportFormat::Json,
        path: export_path.clone(),
    });
    match recv(&handle) {
        Response::Export {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            let summary = result.expect("export");
            assert_eq!(summary.count, 4);
            assert!(export_path.exists());
        }
        other => panic!("expected export, got {other:?}"),
    }

    // Bulk delete + undo through the actor.
    let id = handle.bulk_delete(BookmarkSelection::Ids(vec![target.id.clone()]));
    match recv(&handle) {
        Response::BulkDelete {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(result.expect("bulk delete").affected, 1);
        }
        other => panic!("expected bulk delete, got {other:?}"),
    }
    let id = handle.undo_latest();
    match recv(&handle) {
        Response::UndoLatest {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            assert_eq!(result.expect("undo delete").affected, 1);
        }
        other => panic!("expected undo latest, got {other:?}"),
    }

    handle.shutdown();
}

#[test]
fn typed_actor_errors_are_data_not_panics() {
    let handle = actor();
    let id = handle.set_tag_color("rust", "not-a-color");
    match recv(&handle) {
        Response::SetTagColor {
            id: response_id,
            result,
        } => {
            assert_eq!(response_id, id);
            let error = result.expect_err("invalid color is a typed error");
            assert!(error.message().contains("#rrggbb"));
        }
        other => panic!("expected set tag color, got {other:?}"),
    }
    handle.shutdown();
}

fn _assert_services_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StoreHandle>();
}
