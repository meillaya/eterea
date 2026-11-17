//! Release-only data-layer benchmarks for the v2 CSV and export paths.
//!
//! The requirement is a 10,000-row CSV parse + write under one second on the
//! reference host. These tests are `#[ignore]` so a debug `cargo test` never
//! fails on timing; run them in release:
//!
//! ```text
//! cargo test -p eterea-core --release --test v2_benchmarks -- --ignored --nocapture
//! ```

use chrono::{TimeZone, Utc};
use eterea_core::ingestion::NewCsvParser;
use eterea_core::models::{Bookmark, BookmarkBuilder, Media, MediaType};
use eterea_core::storage::{BookmarkQuery, ExportFormat, ExportRequest, ExportScope};
use eterea_core::{Database, Ingester};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const COUNT: usize = 10_000;
const CSV_BUDGET: Duration = Duration::from_millis(1_000);

/// Deterministic new-format CSV generated through the real CSV writer so
/// quotes, commas, and UTF-8 are escaped exactly as a producer would escape
/// them.
fn generated_csv(count: usize) -> String {
    let mut writer = csv::WriterBuilder::new()
        .has_headers(true)
        .from_writer(Vec::new());
    writer
        .write_record([
            "profile_image_url_https",
            "screen_name",
            "name",
            "full_text",
            "note_tweet_text",
            "tweeted_at",
            "tweet_url",
        ])
        .unwrap();
    for index in 0..count {
        let handle = format!("author{}", index % 200);
        let text = format!(
            "benchmark row {index} for #tag{} with \"quotes\", commas, héllo 世界",
            index % 50
        );
        writer
            .write_record([
                format!("https://example.com/{handle}.png"),
                handle.clone(),
                format!("Author {handle}"),
                text,
                String::new(),
                "2026-05-01T10:00:00.000Z".to_string(),
                format!("https://x.com/{handle}/status/{index}"),
            ])
            .unwrap();
    }
    let bytes = writer.into_inner().unwrap();
    String::from_utf8(bytes).expect("CSV output is UTF-8")
}

fn generated_bookmarks(count: usize) -> Vec<Bookmark> {
    (0..count)
        .map(|index| {
            let handle = format!("author{}", index % 200);
            BookmarkBuilder::new()
                .tweet_url(format!("https://x.com/{handle}/status/{index}"))
                .content(format!(
                    "benchmark row {index} for #tag{} with \"quotes\", commas, héllo 世界",
                    index % 50
                ))
                .tweeted_at(
                    Utc.with_ymd_and_hms(2026, 5, 1, 10, 0, 0).unwrap()
                        + chrono::Duration::seconds(index as i64),
                )
                .author_handle(handle.clone())
                .author_name(format!("Author {handle}"))
                .add_tag(format!("tag{}", index % 50))
                .add_media_metadata(
                    Media::new(
                        format!("https://pbs.twimg.com/media/{index}.jpg"),
                        MediaType::Image,
                    )
                    .with_dimensions(Some(1200), Some(800)),
                )
                .build()
                .expect("benchmark bookmark builds")
        })
        .collect()
}

#[test]
#[ignore = "release perf gate"]
fn ten_thousand_csv_parse_write_and_export_under_one_second() {
    let csv = generated_csv(COUNT);

    let parse_started = Instant::now();
    let parsed = NewCsvParser::new()
        .parse_str(&csv)
        .expect("generated CSV parses");
    let parse_elapsed = parse_started.elapsed();
    assert_eq!(parsed.len(), COUNT);

    let insert_started = Instant::now();
    let db = Database::open_memory().expect("in-memory db opens");
    assert_eq!(db.insert_bookmarks(&parsed).expect("bulk insert"), COUNT);
    let insert_elapsed = insert_started.elapsed();

    let dir = TempDir::new().expect("temp dir");
    let csv_path = dir.path().join("out.csv");
    let export_started = Instant::now();
    let summary = db
        .export(&ExportRequest {
            scope: ExportScope::Query(BookmarkQuery::default()),
            format: ExportFormat::Csv,
            path: csv_path.clone(),
        })
        .expect("CSV export");
    let export_elapsed = export_started.elapsed();
    assert_eq!(summary.count, COUNT);

    let json_path = dir.path().join("out.json");
    let json_started = Instant::now();
    db.export(&ExportRequest {
        scope: ExportScope::All,
        format: ExportFormat::Json,
        path: json_path.clone(),
    })
    .expect("JSON export");
    let json_elapsed = json_started.elapsed();

    let reimport_started = Instant::now();
    let reimported = Ingester::new()
        .parse_file(&csv_path)
        .expect("exported CSV re-imports");
    let reimport_elapsed = reimport_started.elapsed();
    assert_eq!(reimported.len(), COUNT);

    let parse_and_write = parse_elapsed + export_elapsed;
    eprintln!(
        "v2 benchmark: rows={COUNT} parse_ms={} insert_ms={} export_csv_ms={} export_json_ms={} reimport_ms={} parse_plus_write_ms={}",
        parse_elapsed.as_millis(),
        insert_elapsed.as_millis(),
        export_elapsed.as_millis(),
        json_elapsed.as_millis(),
        reimport_elapsed.as_millis(),
        parse_and_write.as_millis(),
    );
    assert!(
        parse_and_write <= CSV_BUDGET,
        "10k CSV parse + write took {parse_and_write:?}, budget {CSV_BUDGET:?}"
    );
}

#[test]
#[ignore = "release perf gate"]
fn ten_thousand_export_benchmark_is_reported() {
    let db = Database::open_memory().expect("in-memory db opens");
    db.insert_bookmarks(&generated_bookmarks(COUNT))
        .expect("bulk insert");

    let dir = TempDir::new().expect("temp dir");
    for (format, name) in [
        (ExportFormat::Csv, "out.csv"),
        (ExportFormat::Json, "out.json"),
    ] {
        let path = dir.path().join(name);
        let started = Instant::now();
        let summary = db
            .export(&ExportRequest {
                scope: ExportScope::All,
                format,
                path: path.clone(),
            })
            .expect("export");
        let elapsed = started.elapsed();
        assert_eq!(summary.count, COUNT);
        eprintln!(
            "v2 export benchmark: format={:?} rows={} bytes={} elapsed_ms={}",
            format,
            summary.count,
            summary.bytes,
            elapsed.as_millis()
        );
        let _ = std::fs::remove_file(path);
    }
}
