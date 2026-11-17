# Verification specification

Numbered invariants for eterea, derived from `docs/design/SPEC.md` (the design
source of truth) and the rewrite plan. This file says *what should be true*.
`verification/COVERAGE.md` says *what is actually checked, how, and what is
merely trusted*.

The invariant ids are stable: harnesses, Kani proof names, Verus lemmas and the
Rune UI harness report them verbatim. Prefixes: `IMP` import, `QRY` query and
selection, `MED` media and URL policy, `UI` interface model and keyboard,
`GEO` geometry and layout, `RND` rendering, theme, motion and accessibility.

## Status vocabulary

| Term | Meaning |
| --- | --- |
| **proved** | Deductive machine-checked proof (Verus) of a model that faithfully represents the code being discussed. Today this exists only for the reference URL model, not for `src/logic` itself. |
| **model-checked** | Bounded exhaustive check of the *actual* Rust function with Kani over a stated input bound. |
| **runtime-validated** | Concrete execution: the harness-crate tests, Miri UB detection, and (once built) the headless Rune UI harness. Passing means "not observed to fail on these inputs". |
| **trusted** | Relied upon but not checked by us: a third-party crate, the OS, or human review. Listed so nobody mistakes presence in the build for verification. |
| **not started** | The invariant is specified here but no check exists yet. This is the honest default once an owner is known. |

A check never upgrades silently: `COVERAGE.md` names the harness and its most
recent observed result.

---

## 1. Import (`INV-IMP-*`)

Sources: `docs/design/SPEC.md` §4 import, §7 data and privacy.

- **INV-IMP-01 Formats.** The importer accepts CSV, JSON and an X archive
  export, and rejects everything else with one sentence of copy. No partial
  database write on a rejected file.
- **INV-IMP-02 Preview never writes.** Parsing a file for preview performs no
  durable mutation: no bookmarks, no media rows, no migrations beyond the
  idempotent schema creation, no dedup marking.
- **INV-IMP-03 Dedup key.** The duplicate identity is the raw `tweet_url`; two
  rows with the same URL are one bookmark. Duplicate counts reported in the
  preview equal duplicates skipped by the import.
- **INV-IMP-04 Quote honesty.** A quoted post without a stored body becomes an
  unavailable quote carrying its URL (`not in archive` in the UI); the importer
  never invents quote text, author or date.
- **INV-IMP-05 Media metadata only.** Import stores media URLs, dimensions,
  durations, poster URLs and variants. No media bytes are ever stored in
  SQLite; nothing is fetched during import.
- **INV-IMP-06 No synthetic fields.** Import produces no engagement counts,
  reach, verification badges or other fields absent from the source file.
- **INV-IMP-07 Preserve source order.** Within one saved day the importer
  preserves the source's chronological order; `posted_at` is parsed, not
  generated from import time.
- **INV-IMP-08 Idempotent schema.** Migrations are idempotent and safe on an
  existing library; schema version is recorded and downgrades are refused.

## 2. Query, filtering and selection (`INV-QRY-*`)

Sources: `docs/design/SPEC.md` §3 selection, §4 feed/search/authors/topics/
graph/calendar, §7 data shape.

- **INV-QRY-01 Selection exists.** Once a feed has at least one post, exactly
  one post is selected; the first post is selected on load.
- **INV-QRY-02 Move order.** `j`/`down` selects the next post in display order
  and `k`/`up` the previous; movement clamps at the ends and never produces an
  empty or double selection.
- **INV-QRY-03 Detail does not lose position.** Opening detail keeps the
  selected post and `Esc`/`Backspace` returns to the previous feed scroll
  position; `j`/`k` in detail move through the same order.
- **INV-QRY-04 Filter totality.** Search, author and tag filters return a
  subset of the loaded model with display order preserved; clearing a filter
  restores the unfiltered order exactly.
- **INV-QRY-05 Counts are data counts.** Any count shown (authors, topics,
  calendar week, status line) is computed from stored rows, never estimated or
  invented; chrome shows no counts that the specification deleted.
- **INV-QRY-06 Pagination stability.** Loading the next page appends without
  duplicating an already loaded bookmark and without changing the selection.
- **INV-QRY-07 Empty and no-match states.** Empty archive, empty filter result
  and end-of-feed render the specified single-line state, not the error state.
- **INV-QRY-08 Read-only views.** Feed, search, authors, topics, graph and
  calendar cannot mutate the library; only favorite/note/import paths write.

## 3. Media and URL policy (`INV-MED-*`)

Sources: `docs/design/SPEC.md` §7 data and privacy; `src/logic/src/lib.rs`;
`docs/verification/rune-spike.md` for the rendering machine.

- **INV-MED-01 Scheme.** A URL is accepted only if its scheme is HTTPS. RFC 3986
  makes the scheme case-insensitive, so `HTTPS://` and `hTtPs://` are HTTPS;
  `HTTP`, other schemes and scheme-less strings are rejected (fail closed).
- **INV-MED-02 Public host only.** An accepted URL's host is not loopback,
  private, link-local, unique-local, multicast or unspecified — including IPv6
  literals, IPv4-mapped IPv6 forms, trailing-dot names and non-canonical IPv4
  spellings that resolvers accept. The last class was finding F-1, fixed and
  regression-locked on 2026-11-17 (`FINDINGS.md`).
- **INV-MED-03 No credentials.** An accepted URL carries no userinfo
  (`user:pass@` or `user@`) in its authority.
- **INV-MED-04 Host presence and shape.** An accepted URL has a non-empty,
  syntactically plausible host; empty or query-only authorities, fragments,
  control/whitespace characters and invalid ports (0, >65535, non-numeric) are
  rejected. Host length is bounded (≤ 253 bytes after trailing dots).
- **INV-MED-05 Explicit load.** Post media is not fetched until the user loads
  media for that post or enables media for the session. Avatars are the only
  default fetch, and the setting can disable them.
- **INV-MED-06 Request hygiene.** Remote media requests send no cookies and no
  referrer, use a stable user agent, a bounded timeout and response-size cap,
  and refuse redirects whose final URL fails INV-MED-01..04.
- **INV-MED-07 Cache discipline.** Disk cache is opt-in, keyed by content,
  bounded, clearable, and never stored in SQLite.
- **INV-MED-08 Cancellation and bounds.** In-flight media requests can be
  cancelled by id; a late result is dropped or marked cancelled; queues and
  worker counts are bounded.
- **INV-MED-09 Image MIME allowlist.** A remote image is decoded only when its
  MIME essence is one of the formats eterea supports (`image/jpeg`, `image/png`,
  `image/gif`, `image/webp`, `image/avif`), matched case-insensitively with
  `;`-parameters ignored; SVG, HTML, video and lookalike types are rejected.

Evidence for the current policy's known deviations: `verification/FINDINGS.md`.

## 4. Interface model, keyboard and state (`INV-UI-*`)

Sources: `docs/design/SPEC.md` §3 modes/keys/selection/status, §4 screens.

- **INV-UI-01 Single-key table.** Normal-mode single-key actions match the SPEC
  table exactly: `j`/`down` next, `k`/`up` previous, `enter` open, `escape`
  back, `f` favorite, `c` copy link, `o` open on X, `m` load media, `/` search,
  `:` command, `?` keys, `,` settings, `space` play/pause. No other single key
  produces a global action.
- **INV-UI-02 Sequence and mode layer.** `Shift-j`/`Shift-k` extend the
  selection; `g` then `g` maps to top, `G` maps to
  end, `←`/`→` apply only to focused media, `Space` only when media is focused.
  These are not claimed by the single-key layer.
- **INV-UI-03 Modes and no leaks.** Normal/Insert/Command/Search are explicit;
  a focused text field swallows keys so no global shortcut fires while typing.
- **INV-UI-04 Esc never traps.** `Esc` closes the top overlay, then search,
  then leaves detail, then clears a panel; it never dead-ends.
- **INV-UI-05 Mode gating.** Overlays (palette, key sheet, lightbox, tags, bulk tag, delete confirm, export, graph colors) consume
  navigation keys and restore the underlying mode and selection on close.
- **INV-UI-06 No accidental Ctrl/Cmd.** Only palette, search, settings and
  OS-reserved clipboard/quit shortcuts use Ctrl/Cmd; no other global shortcut
  does.
- **INV-UI-07 Key sheet cannot drift.** The `?` key sheet lists every globally
  bound normal-mode key and is checked by the headless capture; any new binding
  must update the sheet and the Kani keymap corpus together.
- **INV-UI-08 Status lifecycle.** The status line exists only while it has a
  message, clears after 2 s or on the next action, and never shows permanent
  hints, modes, version strings or counts.
- **INV-UI-09 Settings are not SQLite.** Theme, text size, motion, avatars,
  cache and window geometry persist in `settings.json`; settings never write
  the library database.

## 5. Geometry and layout (`INV-GEO-*`)

Sources: `docs/design/SPEC.md` §2 layout and density, §4 media.

- **INV-GEO-01 Window.** Default 1280x820, minimum 960x640, decorated system
  window, size and position remembered, title `eterea`.
- **INV-GEO-02 Reading column.** Feed reading column is 720 px centred;
  content starts 64 px below the 56 px header; header side padding is 52 px.
- **INV-GEO-03 Marginalia rail.** Rail is 150 px wide, right-aligned, ending
  30 px left of the reading column.
- **INV-GEO-04 Post rhythm.** 64 px between posts; a 1 px `line` appears only
  where the saved day changes and spans the reading column, never between every
  post.
- **INV-GEO-05 Selection rule.** A 2 px `ink` rule sits 14 px left of the
  reading column for the full selected post height; there is exactly one.
- **INV-GEO-06 Media slots.** One image 720x420; two 358x252; three 720x360 plus
  two 358x252; four 2x2 at 358x252; 3 px gaps; 2 px radius; aspect ratio always
  reserved before bytes arrive.
- **INV-GEO-07 Detail width.** Detail uses the same column, 760 px only when a
  gallery needs the room; short detail is vertically centred.
- **INV-GEO-08 Text scale.** Text size setting scales post and detail text only
  (100/112/125 %) and never clips or reflows chrome; layouts reflow at 125 %.
- **INV-GEO-09 Density is fixed.** Exactly one density; no compact/comfortable
  variant exists.

## 6. Rendering, theme, motion and accessibility (`INV-RND-*`)

Sources: `docs/design/SPEC.md` §2 surfaces/type, §5 motion, §6 accessibility.

- **INV-RND-01 One accent.** Links, focus, selection, favorite pin and active
  calendar week are the only accent uses; brand vermilion never appears in app
  chrome.
- **INV-RND-02 No decoration.** No gradients, glass, glows or shadows in app
  chrome except the brand orb and the lightbox scrim; no card/border/shadow
  around posts.
- **INV-RND-03 Motion budget.** Only interaction feedback animates; nothing
  animates on the first frame; durations stay within §5; reduced motion sets
  every duration to 0 except a 120 ms overlay opacity fade.
- **INV-RND-04 Contrast.** Body text ≥ 4.5:1, large text and controls ≥ 3:1/4.5:1
  on every token pair (light and dark) with the specified faces and sizes.
- **INV-RND-05 Focus visible.** Keyboard focus shows a 1 px accent ring at 2 px
  offset; pointer focus does not; pointer targets are at least 24x24 px for
  text actions and 32 px for nav rows.
- **INV-RND-06 Honest media states.** Automatic media is the default; on-demand
  mode shows the 720x180 band with one `load media` action. A loaded image never
  shifts layout; video poster, play circle and duration match §4; missing media
  shows no invented content. A >4-item post pages the carousel rather than
  truncating; the lightbox covers every item.
- **INV-RND-07 Typography.** Bundled Newsreader, Instrument Sans and IBM Plex
  Mono are used for the specified roles; no all-caps, no letter-spacing, no
  text under 10.5 px; tabular numbers for times, dates, counts and durations.
- **INV-RND-08 Accessible tree.** Every route has a coherent accessibility
  tree (roles and labels for nav, post, author, media, favorite, quote, dialog,
  list, graph, calendar); every image has alt text or is marked decorative;
  every icon-only control has a name.
- **INV-RND-09 Renderer is not a browser.** Text, avatars, images, video and
  quotes render through the Rune surface without a browser engine; CJK, RTL
  and emoji follow the font fallback.

---

## Change rule

Add or edit an invariant here first, then add the check (or an explicit
`not started` row) in `COVERAGE.md` in the same change. A check without an
invariant id and an invariant without a coverage row are both review errors.


## 7. Reader state, history, export and carousel (`INV-DATA-*`)

- **INV-DATA-01 Read state.** Every imported bookmark starts unread; opening a
  post marks it read; read/unread updates are idempotent and survive restart.
- **INV-DATA-02 URL entities.** Own/quote/media tweet URLs are suppressed from
  rendered text; external URLs keep a display URL and expanded HTTPS target;
  unavailable quotes never invent a body or show a raw tweet URL.
- **INV-DATA-03 Media cap and quality.** At most 20 media items per bookmark
  render; image requests use the best safe rendition; video playback chooses a
  progressive MP4 variant (highest bitrate, never HLS); a carousel page renders
  a deterministic contiguous slice.
- **INV-DATA-04 Import batch accounting.** A batch inserts each unique URL
  once, reports inserted/duplicate/error counts, writes one history event, and
  never changes the route.
- **INV-DATA-05 Selection/bulk.** Ids and Query selections apply the same
  operation to every matching row; page/all-matching/clear are explicit; bulk
  tag/delete/read update FTS and counts in one transaction.
- **INV-DATA-06 History/undo.** History keeps the latest five journaled import,
  delete and tag events. Only the newest non-undone event can be undone; its
  inverse is atomic, restores exact pre-action state where possible, and reports
  skipped conflicts. Undone events keep an honest status.
- **INV-DATA-07 Export.** CSV and JSON export are streaming, UTF-8, scoped to
  all/filter/selection, and round-trippable; no media bytes or environment
  paths are included.
- **INV-DATA-08 Tags/FTS.** Adding/removing/setting tags updates tags,
  bookmark_tags, the FTS tags_text and topic/graph queries atomically; deleting
  a bookmark removes its FTS content row.
- **INV-DATA-09 Color overrides.** Per-tag colors accept only validated hex
  tokens, persist, and affect only graph node styling.
- **INV-DATA-10 Startup.** SQLite open, settings read and initial feed request
  do not block the first frame; the release document opens without a missing
  Wayland/X11 library.

## 8. Release and cache (`INV-REL-*`)

- **INV-REL-01 Runtime bundle.** The Linux bundle's direct ELF and launcher load
  their display/font/Vulkan libraries from the bundle or a documented system
  path; a clean-env smoke creates a window or reports an actionable error.
- **INV-REL-02 Cache containment.** Image/video cache filenames are content
  digests, never URLs; cache clear leaves the archive untouched; no media bytes
  are stored in SQLite.
- **INV-REL-03 Audio/decoder absence.** Missing ffmpeg/ffplay is reported as a
  typed actionable error; video never receives a remote URL.

---

## Change rule
