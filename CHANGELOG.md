# Changelog

All notable changes to eterea are recorded here. This project follows a
human-readable changelog and keeps release notes honest about the evidence that
has actually been captured.

## Unreleased

- Duplicate imports now enrich an existing post instead of skipping it: a richer
  export can backfill media, quotes, links, tags and refreshed author
  name/avatar for tweets that were first imported from a text-only CSV, without
  touching read or favorite state or creating a second row.
- Avatars are resolved per account, not per archived row: the newest known
  portrait is shown on every post of that handle, with older renditions tried as
  fallbacks when X has deleted the current URL (404) or a mirror expires.
  Directory rows use the newest post's name and portrait too.
- The selection rule is a short pale accent bar in the rail gap, aligned with
  the byline, instead of a full-height ink bar.
- Clicking a post selects the post under the pointer before opening, tagging,
  loading media or opening the lightbox; it no longer opens the previously
  selected post.
- Image fetches also accept `application/octet-stream` / `binary/octet-stream`
  bodies (some avatar mirrors label JPEGs that way); the decoder remains the
  validator.
- Fixed the reading surface: the feed now carries its 64px post rhythm (same-day
  posts no longer touch), pages the next 48 posts in as you reach the bottom,
  and drops replies from superseded searches so a stale page cannot replace a
  newer filter.
- Made search discoverable: a `search` header item opens the same field as `/`,
  the field shows its `search` placeholder and a `searchbox` role, and results
  page through the whole matching archive instead of one fixed page.
- Redesigned authors and topics: a filter row with a live directory filter
  (`/`), larger two-line author rows with avatars, `#tag` topic rows, hover and
  selection states, loading rows and honest empty states.
- Added a minimizable shortcut tooltip: the footer `keys` hint opens a clean,
  scrimless key sheet anchored bottom right; `esc` or a press outside folds it.
  Corrected the sheet's read/unread row and documented mouse back/forward.
- Added mouse back/forward navigation with a route stack, and fixed status-line
  lifetimes so feed errors stay visible for their full two seconds.
- Fixed the media request path to retry bounded-queue overflows instead of
  blacklisting keys for the session, and clearing the cache now resets slots.
- Added a rendered desktop architecture figure and rewrote the README around
  the current import, library, and service boundary.
- Switched the development shell to `devenv.nix` and removed the duplicate
  flake dev shell and `shell.nix`.
- Reduced the release binary and import path: narrowed shell features, removed
  unused regex and tokio dependencies, cached tag IDs, and made imports run in
  one transaction.
- UI pass: lifted muted-text contrast, kept dates and tags on one line,
  reported loaded counts in the status line, opened details with Enter,
  expanded the dashboard with favorites and timeline panels, and added
  keyboard layout commands.
- Keep release notes aligned with the local-first privacy model documented in
  [release readiness](docs/operations/release-readiness.md).

## 0.1.0 production-candidate

Initial production-candidate desktop release surface.

- Local-first X/Twitter bookmark archive with CSV, JSON, and X archive import
  paths.
- Dry-run import preview before committed writes.
- SQLite-backed library, favorites, search, author/topic directories, detail
  routes, and table, tree, dashboard, graph, and calendar views.
- Rune desktop shell with persisted appearance settings and automatic media
  previews for the visible archive.
- devenv-based verification path documented in [development](docs/development.md).

Known release follow-ups remain tracked in
[release readiness](docs/operations/release-readiness.md), including live desktop
screenshot/timing, installer/signing, and release-grade performance evidence.
