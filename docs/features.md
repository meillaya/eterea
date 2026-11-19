# Features

eterea imports X bookmarks from CSV, JSON or an X archive `.zip` directly, keeps
them in SQLite, and reads the archive with a GPU-drawn desktop shell. Everything
below runs against the local archive: there is no account, no sync and no
server. Keys named in the list are the defaults from the
[shortcut sheet](../README.md#shortcuts); the command palette (`:`) exposes the
same actions.

- Imports CSV, JSON and X archive exports, one file or many, by picker or drop.
  Preview before write; a duplicate tweet URL is not re-inserted, but a richer
  source still backfills media, quotes, links, tags and the account's current
  name and avatar for it.
- Searches text, names, handles and tags over SQLite FTS5. The feed, and with it
  every search result, pages in as you scroll, so the whole archive is reachable
  rather than a demo cap. Author and topic lists filter and page the same way,
  with an inline filter field (`/` while the list is open).
- Loads avatars and media automatically for the visible archive, at the best
  rendition, through a bounded disk cache. Portraits resolve per account: the
  newest known avatar is shown everywhere, with older renditions as fallbacks
  when X has deleted the current one. Images and video files are cached
  outside SQLite; the cache can be measured and cleared in settings. The media
  queue retries keys that arrive while it is full instead of dropping them.
- Renders X media carousels up to 20 items, with a lightbox across the whole set.
  Tweet and quote URLs are removed from the reading surface; external links keep
  their display URL and quotes show as cards.
- Plays progressive MP4 video from the persistent cache with `ffmpeg`, with
  `ffplay` audio when unmuted. Sound starts off; `m` toggles it. Sound and
  picture stay on one timeline across pause, resume and loops, and leaving the
  video (back to the feed, closing the lightbox, another post) stops both.
- Edits tags and deletes posts from the viewer. Marks read or unread, selects a
  page or the whole matching archive, and applies tags, deletion, read or unread
  to the selection.
- Keeps the last five import, delete and tag actions in History. The newest can
  be undone from its exact before-state; older actions stay visible with detail.
- Repairs missing media and quotes on demand: posts imported from a text-only
  export can be filled in from the public embed payload X serves for embedded
  posts (media list, quoted post, expanded links, hashtags, current author
  name/avatar). Opt-in via the `repair media` palette command; the footer shows
  a slim progress bar, `cancel` (or the palette's `cancel repair`) stops it, and
  `esc` keeps doing normal navigation. Deleted and protected posts are skipped,
  media bytes still fill the normal cache when shown, and the pass remembers the
  posts it has checked (schema v8): running it again visits only unchecked posts
  and reports "nothing to repair" when there are none.
- Exports the archive, the current filter or the selection as CSV or JSON, with
  round-trippable nested data.
- Keeps quotes, favorites, media metadata, an interactive tag graph with persisted
  colors, and a calendar month.
- Imports honestly: the X archive `.zip` is read entry by entry, real progress
  shows while files are read and posts saved, `esc` cancels before the write,
  each file in a batch is reported separately (parse failures do not abort the
  rest), and the dry-run duplicate estimate uses exactly the insert's rules.
- Reconstructs threads when the export carries conversation ids (X API v2
  `conversation_id`) and shows the whole thread in detail, oldest first, with the
  opened post highlighted. Archives imported from text-only CSVs carry no
  conversation ids, so the thread view appears only for sources that included
  them (X API v2/archive JSON); filling them in for older archives would need the
  official API with your token, which eterea does not implement.
- Opens media in a lightbox with a `n / m` count, pointer and mouse-back paging,
  wheel/`+`/`-`/`0` zoom from fit to 3×, and video scrubbing with `,`/`.` (±5s,
  restart-at-offset) with a live position/duration readout.
- Loads media only for rows that are actually visible (scrolling loads more) and
  resumes interrupted video downloads with HTTP ranges instead of restarting.
- Keeps filters as reusable saved searches: compose query, tags, author, date,
  read/favorite and media facets into one predicate, save it under a name, and
  remove facets from the chip line; `add tag filter` composes tags.
- Flags posts that X reports as gone: a quiet “no longer available on x” line in
  the feed and detail (with an explanation that the tweet was deleted, protected
  or its account no longer exists). The opt-in `check availability` pass fills
  those flags and can be cancelled; a later successful check clears them.
- Highlights search terms in the feed body with a pale lavender mark, and opens
  the trash (`trash`) to list recoverable deletes by author with selective
  restore or a two-step empty; restoring a backup asks for confirmation.
- Curates tags: `rename tag` renames a tag on the selected post (merging into an
  existing tag folds both together, with autocomplete suggestions), `prune tags`
  removes unused tags, and per-tag colours can be reset to their seeded colour.
- Lets you read in order: `cmd-n` / `next unread` opens the next unread post and
  marks the current one read; when the view runs out it switches to the unread
  filter and starts again.
- Keeps private notes: `n` (or `edit note`) opens a note editor for the selected
  post; notes appear in the detail view, search across them, and round-trip
  through CSV/JSON exports.
- Makes deletes recoverable: every delete keeps a full snapshot in a bounded
  trash (`restore last deleted` in the palette), and undo (`u`) walks back
  through recent actions instead of stopping at one.
- Protects the archive: a consistent copy is written before every migration
  pass (the newest five rotate), `backup now` makes a manual copy, a palette
  `check integrity` runs SQLite's integrity and foreign-key checks, and
  `restore backup` validates a chosen copy byte-for-byte before replacing the
  live archive page-wise.
- Browses the archive on a calendar: month grid with a selected-week rule and
  per-day marks, a year overview of twelve month plates whose dots shade with
  saved-post density, `‹ ›` month chevrons, `[`/`]`, `t` for today and `y` for
  the year view.
- Remembers the window's size and position, refuses a second instance on the
  same profile, writes `settings.json` atomically, and shows the database path,
  backup count, tool availability and a retry for media that hit a transient
  network failure.
- `g g` / `G` jump to the top/end of the feed, a visible `filtered: …` line
  says when a filter is on and offers `clear`, and `retry media` re-requests
  images that failed for network reasons.
- Switches between system, light and dark appearance, three text sizes, automatic
  or on-demand media, and system or reduced motion. Preferences persist.

The evidence behind these claims is indexed in
[`verification/COVERAGE.md`](../verification/COVERAGE.md): plenty of it is
runtime validation, not proof. Feature depth is also listed as a backlog in the
[2026-11-17 critical audit](audit/2026-11-17-critical-audit.md).
