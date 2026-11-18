# Changelog

All notable changes to eterea are recorded here. This project follows a
human-readable changelog and keeps release notes honest about the evidence that
has actually been captured.

## Unreleased

- Lightbox: a mono `n / m` count, clickable side zones plus mouse back/forward
  paging, and wheel/`+`/`-`/`0` zoom between fit and 3× (zoom resets when paging
  or closing), plus video scrubbing with `,`/`.` (±5 s, decoder restarts at the
  offset) and a live position/duration readout while a video plays.
- Graph hit testing: dots are drawn in one canvas batch and hover/click use a
  spatial hit test against the same projection, replacing per-node hit elements
  (labels stay text). Video scrubbing gets a draggable timeline that previews the
  target and seeks once on release, alongside the live position/duration.
- Graph finish: the layout builds 1,200 tags through a co-occurrence query
  restricted to the selected node set (no archive-wide pair scan), labels skip
  anchors that would collide, and `/` opens a tag filter that dims non-matching
  nodes (enter commits, esc clears); a synthetic 1,200-tag benchmark test covers
  the data-layer cap (2.1s in debug, layout included).
- Search highlighting: when a text query is active, matching terms in feed and
  detail bodies get a pale lavender mark (ASCII-insensitive, up to 40 spans).
- Trash browser and restore safeguard: `trash` lists the newest recoverable
  deletes with author, preview and date, Enter/restore reinstates one, `e` twice
  empties the trash, and restoring a backup now opens a confirmation panel
  (Settings also exposes restore next to backup/integrity). Rotating automatic
  copies continue to be taken on every launch.
- Saved searches and composable filters: the feed filter is now a composable
  spec (query, tags, author, date range, read, favorite, media), shown as a chip
  line with per-facet removal; named combinations save to `app_metadata` via
  `save search` and reopen from `saved searches` (apply or delete), and
  `add tag filter` composes tags. Backend bookmarks queries accept multiple
  tags (AND-composed) and are covered by composed-filter tests.
- Availability indication: schema v7 stores a per-bookmark `unavailable` flag
  set by the opt-in repair/`check availability` pass when X's embed payload is a
  tombstone (deleted tweet, protected author, deactivated account) and cleared
  when a later fetch succeeds; feed and detail show a quiet "no longer available
  on x" line (detail also explains deleted/protected/account gone).
- Tag management: `rename tag` (+ autocomplete, merging into an existing tag),
  `prune tags` for unused tags, and a per-tag colour reset that deletes the
  stored override; renaming carries the persisted colour to the new name and
  keeps the FTS tag document in sync.
- Reading queue: `cmd-n` / `next unread` opens the next unread post and marks
  the current one read, falling back to the unread filter when the current view
  has no unread posts left.
- Private notes: `n` / `edit note` opens a per-post note editor (save with the
  button or cmd-enter, clear to remove); notes render under the post body in
  detail, are searchable through FTS, and already round-trip through CSV/JSON
  exports.
- Search index coverage (schema v5): quoted-post text, media alt text and
  expanded link targets are indexed alongside the body, with the document table
  and FTS postings rebuilt during migration and refreshed when a duplicate
  import enriches an existing row with a quote, media or links.
- Import honesty: X archive `.zip` files are imported directly (entries are
  parsed individually, malformed ones skipped), imports report real progress
  (`reading files n/m · rows`, then `saving posts`), `esc` cancels cooperatively
  before anything is written and reports the cancellation, per-file parse
  failures no longer abort the batch and are listed in preview and summary, and
  the preview duplicate count now matches the commit exactly (existing URLs and
  ids intersected with intra-file and intra-batch repeats).
- Polish: `g g`/`G` top/end of feed, a visible `filtered: …` line with a
  `clear` action, persisted window size/position, a single-instance guard,
  atomic `settings.json` writes with status on failure, a Settings safety
  section (database path, backup count + actions, ffmpeg/ffplay availability,
  source link), transient media failures retried twice before being marked
  unavailable (plus a `retry media` command), and a contrast-gated muted colour
  (4.67:1 on light).
- Search results for a text query are now ordered by FTS `bm25` relevance rather
  than recency, while filters keep their usual order. Page totals are counted
  separately, so a page scrolled past the last result reports the real total
  instead of zero.
- Recoverable deletes: single and bulk deletes snapshot the post into a 200-entry
  trash inside the same transaction (new schema v4 table with a pre-migration
  backup), `restore last deleted` restores the newest entry, and repeated `u`
  keeps walking the undo journal.
- Data safety: a consistent pre-migration copy is written on every open of an
  existing archive (newest five kept), `backup now`/`list backups`/`restore
  backup`/`check integrity` are palette commands, restores validate the copy's
  integrity and schema version first and swap it in through SQLite's online
  backup API, and old copies are migrated forward on restore.
- The graph now uses the whole content area instead of a fixed 720px square:
  wheel/pinch zoom and pan are clamped so the constellation cannot be cropped,
  `0`/double-press fit it, and the deterministic layout is force-directed rather
  than a plain circle, scaled to the field's aspect ratio. Up to 1,200 tags and
  6,000 edges are supported; labels are level-of-detail (140 tags label
  everything, larger graphs label the 36 most-saved plus the focused tag).
- Reworked the calendar into a real browser: a header with month chevrons and a
  year-overview toggle, a selected-week accent rule with the week's data count,
  clickable per-day marks that open the post, keyboard week/month/year
  navigation (`←/→`, `↑/↓`, `[`/`]`, `y`, `t`, `Enter`), cached months and a
  twelve-plate year overview whose dot shading shows saved-post density.
- Returning from a post now restores the feed exactly where it was: the opened
  post is remembered as a layout anchor (or the raw scroll offset when the
  selection was off screen), so `esc`/mouse-back land on the same spot even
  after avatars or media finish loading. Returning to the cached feed no longer
  re-queries page 0.
- Added an opt-in `repair media` palette command: rows imported without media
  are filled from X's public embed payload (media and video variants, quoted
  post with its carousel, expanded links, hashtags, current author name and
  avatar) and merged into the existing row without touching read/favorite
  state. The footer shows a slim progress bar with `visited/total` and a
  `cancel` action (also `cancel repair` in the palette); `esc` stays normal
  navigation. The pass is paced, resumable and skips deleted/protected posts;
  media bytes are cached lazily, not pre-downloaded.
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
