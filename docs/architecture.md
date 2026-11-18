# Architecture

eterea is a Rust workspace with one GPU-drawn desktop shell and a shared service boundary. There is no webview.

## Crates

| Path | Crate | Responsibility |
| --- | --- | --- |
| `src/backend` | `eterea-core` | Domain models, CSV/JSON/X archive parsers, SQLite storage and migrations, FTS5 search, statistics, CLI. |
| `src/app` | `eterea-app` | `AppServices` product operations and the background `StoreHandle` actor. The only data boundary the UI uses. |
| `src/logic` | `eterea-logic` | Pure functions: URL policy, MIME policy, text/media helpers. No I/O. Verus/Kani target. |
| `src/ui-model` | `eterea-ui-model` | Pure route, mode, selection and keymap state. No I/O or framework types. Verus/Kani target. |
| `src/media` | `eterea-media` | Bounded HTTPS image workers, optional byte cache, validated video download and local ffmpeg playback. No UI dependency. |
| `src/desktop` | `eterea-desktop` | Rune shell: theme, views, input, media slots, window. Binary `eterea`. |
| `verification/` | harnesses | Kani proofs, Miri tests, Verus reference models and coverage/SPEC docs. |

## Runtime flow

The Rune view sends typed requests to the `StoreHandle` actor; the actor owns
SQLite through `AppServices` and replies with a `RequestId`. Image requests go
to the bounded media workers, which fetch HTTPS bytes for Rune's image cache.
Media bytes never reach SQLite.

## Trust boundaries

Imported archives are untrusted: post text, handles, tags, URLs, alt text, quotes and media URLs are rendered as text and never as HTML. One optional feature crosses the network for metadata after import: the `repair media` command fetches X's public embed payload (`cdn.syndication.twimg.com`, no account, cookies or token) for posts that have no media, paced and cancellable, and merges only additive metadata into existing rows. It is never automatic and it never pre-downloads media bytes. Remote media is fetched only over safe HTTPS (no credentials, fragments, private hosts or non-canonical numeric addresses), without cookies or referrer, with byte and time caps. Opening a stored link requires an explicit action. Quote posts without a body stay marked unavailable rather than invented.

## Source layout

- SQL stays in `src/backend/src/storage/`; parsers stay in `src/backend/src/ingestion/`.
- Product operations stay in `src/app/src/services/app.rs`; the actor lives in `src/app/src/store.rs`.
- Pure UI decisions belong in `src/ui-model`; Rune elements stay in `src/desktop/src/`.
- Design tokens and screen contracts live in `docs/design/SPEC.md`.
- Verification invariants live in `verification/SPEC.md`; coverage is in `verification/COVERAGE.md`.

## Persistence

The default database is `<platform data dir>/eterea/bookmarks.db`. SQLite runs in WAL mode. Schema version is `PRAGMA user_version = 3`; migrations are idempotent, reject newer versions, and only touch the database after the version check. Migration v3 renames the FTS content table from `bookmarks_fts_content` to `bookmarks_search_docs` and rebuilds the external-content index, because the old name was an FTS5 shadow-table name: SQLite newer than the bundled 3.46 refused to parse the whole schema ("cannot create triggers on shadow tables"). Media bytes and settings are not stored in the database. Appearance and media preferences live in `settings.json` under the platform config directory.

## Deferred

Live X sync, multiple accounts, server mode and signed installers are not shipped. Do not describe them as runtime behavior until implemented and verified.

## v2/v3 reader state, history and media

Schema v2 adds `bookmarks.is_read`, `bookmarks.links_json`, `media.playback_url`,
`tag_colors` and `history_events`. The backend owns the data and round-trip CSV/
JSON formats; the app store actor owns typed commands for read/tag/bulk/history/
export operations; the desktop shell owns presentation and pending UI state.
Journaled actions (import, delete, tag) write their inverse payload in the same
SQLite transaction and keep only the latest five events. Only the newest
non-undone event is undoable.

Schema v3 only changes the FTS storage names; every v2 column and row is
preserved, and the migration is a no-op on reopen.

The media crate resolves best progressive MP4 variants, stores played videos in
a bounded content-addressed cache, and spawns external `ffmpeg`/`ffplay` for
frames and audio. Image workers use the same cache directory for image bytes.
The desktop never stores media bytes in SQLite and never sends a remote URL to
ffmpeg/ffplay. URL entities are imported with the bookmark so the reader can
suppress own/quote tweet links, render external display URLs and show honest
quote cards.

The Linux release packager collects Rune's runtime-loaded display/font/Vulkan
libraries, sets `$ORIGIN/lib`, installs desktop-entry icons and smoke-tests the
bundle under Xvfb/lavapipe. A bare `target/release/eterea` still expects the
development shell's library path and is not the distributable.
