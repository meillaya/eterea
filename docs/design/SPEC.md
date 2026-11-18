# eterea design specification

Approved direction: **Daylight** visual system with **Signal** keyboard-first interaction. Source of truth for the Rune build. The Daylight prototype at `design-demos/dir-1.html` is the visual reference; the Signal prototype at `design-demos/dir-2.html` is the interaction reference. Where they differ, this document decides.

## 1. Product

eterea is a local reading app for an X/Twitter bookmark archive. It imports a CSV, JSON, or X archive, stores posts in SQLite, and renders text, avatars, images, video, and quote posts without a browser engine. It is a desktop app for one reader, not a web service.

## 2. Visual system

### Surfaces

| Token | Light | Dark |
| --- | --- | --- |
| `bg` | `#f7f6f3` | `#141412` |
| `ink` | `#1a1a18` | `#ecebe4` |
| `muted` | `#787770` | `#98958b` |
| `line` | `#dedcd6` | `#2a2a25` |
| `accent` | `#2447b8` | `#8fa8ff` |
| `quote rule` | `#c9c6bf` | `#3a3a33` |
| `band` | `#efeee9` | `#1c1d19` |
| `scrim` | `rgba(13,13,12,.86)` | `rgba(5,5,5,.9)` |

- One accent only: links, focus, selection, the favorite pin, active calendar week.
- The brand vermilion `#b4452f` appears only in the app icon and the README, never in the app chrome.
- No gradients, glass, glows, or shadows except the lightbox scrim.
- Light is the default when the system preference is unknown. System, light and dark are selectable.

### Type

| Role | Face | Size / line |
| --- | --- | --- |
| Wordmark | Newsreader 400 | 20 / 20 |
| Post text | Newsreader 400 | 21 / 31 |
| Quote text | Instrument Sans 400 | 14 / 21 |
| Name | Instrument Sans 500 | 13 / 18 |
| Handle, nav, buttons | Instrument Sans 400/500 | 12.5 / 16 |
| Rail date | Instrument Sans 400 | 11.5 / 16 |
| Rail time, keycaps, metadata | IBM Plex Mono 400 | 10.5 / 15 |
| Status | Instrument Sans 400 | 11.5 / 16 |
| Detail body | Newsreader 400 | 22 / 32 |
| Empty state | Newsreader 400 | 24 / 32 |

- Bundle Newsreader, Instrument Sans, IBM Plex Mono (OFL). Fall back to Noto Serif/Sans CJK and the system emoji font.
- No all-caps labels. No letter-spaced labels. No text under 10.5px. Body copy never under 14px.
- Tabular numbers for times, dates, counts and media durations.

### Layout

- Window: 1280x820 default, minimum 960x640. Remember size and position.
- Header: 56px tall, 52px side padding. 20px translucent purple orb + wordmark 20px at the left; nav 12.5px at the right with 26px gaps: feed, authors, topics, graph, calendar, import, history, settings, search.
- Footer: 24px tall, 52px side padding. A transient status message at the left; the `keys` shortcut hint at the right. The hint is the minimizable form of the key sheet: the sheet is a clean tooltip with no scrim, and pressing anywhere outside or `esc` folds it back to the hint.
- Feed: reading column 720px centered. Marginalia rail 86px wide, right-aligned, ending 12px left of the column. The 150px rail ending 30px left from the Daylight prototype does not fit a 720px column inside the 960px minimum window, so the rail stays narrow; a wider rail is a follow-up once the column is responsive. Content starts 64px below the header.
- Post rhythm: 64px between posts, carried as 32px above and below each post. A saved-day change puts one 1px `line` inside that rhythm; it does not add to it.
- Selection: 2px pale accent rule (32% accent) in the 12px rail gap, 18px tall and aligned with the byline's centre. It is not full post height. Selected post scrolls into view.
- Detail: same column, 740px when media needs the room, vertically centered when short. Back returns to the previous scroll position by anchoring the opened post where it was drawn, so avatars or media finishing above it never move the reader's spot. Viewer tags are editable, the last line carries `edit tags` and `delete`; delete confirms before writing.
- Media: column width. One image 720x420; two 358x252 with 3px gaps; three: one 720x360 plus two 358x252; four: 2x2 at 358x252. Radius 2px. Always reserve the aspect ratio.
- Quote: 18px indent, 1px `quote rule` on the left, 14/21 Instrument text, optional 72px thumbnail on the right. No label, no card, no rounded box.
- Right/left margins stay empty. Do not add a sidebar or inspector.

### Density

One density. Do not add compact/comfortable variants. Text size setting scales the post and detail text only (100/112/125%), not the chrome.

## 3. Interaction model

Signal's keyboard-first behavior inside the Daylight surface.

### Modes

| Mode | Entry | Behavior |
| --- | --- | --- |
| Normal | default, `Esc` | Selection and navigation. No text input. |
| Insert | focus a text field | Field owns keys. Global shortcuts do not leak. |
| Command | `:` | Palette overlay, filter, arrow keys, Enter, Esc. |
| Search | `/` | Header search field. Typing filters the feed. Enter opens the first result. Esc clears and closes. |

No visual/multi-select mode. One selected post at a time.

### Keys

| Key | Normal | Detail | Overlay |
| --- | --- | --- | --- |
| `j` / `↓` | next post | next post | palette down |
| `k` / `↑` | previous post | previous post | palette up |
| `Enter` | open post | open media when focused | run command |
| `Esc` | clear search/panel | back to feed | close overlay |
| `/` | search | search | no |
| `:` | command palette | command palette | no |
| `?` | key sheet | key sheet | close |
| `f` | favorite | favorite | no |
| `c` | copy link | copy link | no |
| `o` | open on X | open on X | no |
| `m` | load media, or toggle video sound | toggle sound | no |
| `Space` | play/pause video | play/pause | no |
| `←` / `→` | page the media carousel; move in the lightbox | gallery | no |
| `x` | select and enter selection mode | select | no |
| `Shift-j` `Shift-k` | extend selection | select | no |
| `Cmd/Ctrl-A` | select current page | select page | no |
| `Shift-Cmd/Ctrl-A` | select all matching the filter | select all | no |
| `t` | edit tags; bulk tag in selection | tag | no |
| `d` | delete with confirmation | delete | no |
| `u` / `r` | mark unread / read | read state | no |
| `h` | history and undo | history | no |
| `e` | export CSV/JSON | export | no |
| `,` | settings | settings | no |
| `g` then `g` | top of feed | top | no |
| `G` | end of loaded feed | end | no |
| mouse back | close overlay or text mode, then previous route | back to feed | close |
| mouse forward | return to the route back left | no | no |

- `Esc` never traps. It closes the top overlay, then search, then leaves a non-feed route, then clears the feed filter.
- Text fields swallow keys. The palette, search and the directory filter are the only text contexts outside import and settings.
- No global shortcut uses Ctrl or Cmd except `Ctrl/Cmd-P` for the palette, `Ctrl/Cmd-F` for search, `Ctrl/Cmd-,` for settings, and clipboard/quit shortcuts reserved by the OS.
- Mouse back and forward buttons are first-class: back closes the top overlay or text mode, then returns to the previous route; forward returns to the route back left. Both walk the same route stack the keyboard leaves.
- On authors and topics, `/` focuses the directory filter instead of opening feed search.
- The key sheet is hand-maintained next to the handler. Generating it from one shared keymap table is a tracked follow-up: `eterea_ui_model::action_for` exists but the shell does not consume it yet.

### Selection

- The feed always has a selection once it has posts. The first post is selected on load.
- `j`/`k` moves and scrolls the selected post into view.
- Clicking a post selects the post under the pointer; clicking the byline opens the author filter for that post's author; clicking the post body or its quote card selects it and opens detail; clicking media selects it and opens the lightbox.
- The favorite marker is a 6px accent square in the marginalia, shown only when favorited.
- The rail actions appear on hover or focus; they are text, never icons.

### Status

- 24px footer, 11.5px status text at the left, 52px inset. The status exists only while it has a message: `link copied`, an import result, or an error. It clears after 2s or on the next action.
- The footer's right side carries one permanent hint, `keys`, the minimizable key sheet. It is the only permanent hint; no mode badges, counts, line/column numbers, theme names, or version strings.
- While the opt-in media repair runs, the footer's left side becomes a progress line: `repairing media`, a 180x4px rounded track with a light purple (lavender) fill, `visited/total` in 10.5px mono, and a `cancel` action on the right next to `keys`. `esc` is not overloaded: it never cancels the repair.

## 4. Screens and components

### Feed

Post: rail (date, time, favorite pin, actions), byline (30px avatar, name, handle), text, media, quote. No card, no border, no shadow. Links are accent text underlined on hover. Hashtags and mentions are links to filtered views. Emoji render in color. CJK and RTL follow the font fallback.

Threads: when the source carried a conversation id (X API v2 `conversation_id`), detail lists every saved post of that conversation oldest first — the opened post renders fully, the rest as compact byline+text rows — under a mono `thread · n posts` label. Without thread ids the detail stays a single post.

Media: avatars, images and video posters load automatically by default, fading in 180ms; Settings can switch post media to on-demand and keep the 720x180 `band` with `load media` for that mode. Images use the best available rendition through a bounded disk cache. A post with up to 20 media renders a paged carousel (1/2/3/4 layouts; >4 pages in groups of four with `‹ n/m ›`); the lightbox covers every item. X and quote URLs are removed from the text; external links keep their display URL; quote cards use the same media rules.

Video: poster fills the media slot, centered 44px hairline play circle, 10.5px mono duration at bottom right. Playback resolves the best progressive MP4 variant, caches the file persistently, and uses `ffmpeg` for frames. Inline playback is muted and looping in the feed. Detail and lightbox carry sound; `m` toggles it, and `ffplay` supplies audio from the cached local file. Missing ffmpeg/ffplay is a status error, not silence.

Images: click or Enter opens the lightbox. The lightbox is `scrim` with the image centered, close at top right, previous/next controls at the sides and in the bottom bar with a mono `n / m` count. `+`/`-`/wheel zoom (0.5–3×) and `0` resets to fit; mouse back/forward and the side zones page too; Esc closes. Videos show a draggable timeline (200×3px, previewed while dragging and seeked on release), live position / duration, and `,` / `.` for ±5 s; seeking restarts the decoder at the offset, clamped to the file. No captions.

Empty feed: one line `nothing here yet` in Newsreader 24, one action `import a bookmarks export`. No ASCII, no illustration.

No matches: `no matches`. One line, muted.

Errors: one sentence in the status area, then the next action, e.g. `this file is not a bookmarks export. choose a CSV, JSON or X archive.`

### Detail

Same column; 740px when a gallery needs it. Post, media at full size, quote, editable tags, metadata in 10.5px mono at the bottom (`saved 24 aug 2025`, source). Actions: favorite, copy link, open on X, edit tags, delete, load media when on demand. Back with `Esc`, `Backspace`, or the wordmark. `j`/`k` move to the next/previous post without leaving detail. Opening marks the post read.

### Header search

`/` or `Ctrl/Cmd-F` reveals a 360px field at the right, replacing the nav with a 160ms fade. The nav's rightmost item is the word `search`, so the field is discoverable with a pointer as well; it opens the same field. Placeholder `search`. Typing filters the feed over text, name, handle, tags. Enter opens the first result. Esc clears and closes. The feed pages as it scrolls, so a broad query reaches every match. No result count, no "results" label. A palette command `unread` filters unread posts.

### Command palette

`:` or `Ctrl/Cmd-P`. 560px wide, top 80px, centered, `bg` panel with a 1px `line` border, 2px radius. One field, one list. Commands, lowercase, no descriptions, keycaps only where a shortcut exists: feed, favorites, authors, topics, graph, calendar, import, settings, favorite, load media, copy link, open on x, clear filter, reload, repair media, cancel repair. `repair media` is the only command that touches the network after import; it reports progress in the footer and `cancel repair` (shown only while one is running) stops it. Filter with substring; arrows move; Enter runs; Esc closes. No footer hints.

### Key sheet

`?` or the footer's `keys` hint. A clean tooltip anchored at the bottom right, 320px, 1px `line`, 2px radius, no scrim. A `keys` header row carries `esc`; a press anywhere outside folds it back to the hint. Rows: `j k move`, `enter open`, `esc close`, `/ search`, `mouse back back`, `mouse forward forward`, `f favorite`, `c copy link`, `o open on x`, `m media / sound`, `← → carousel`, `space play`, `x select`, `shift j k extend`, `cmd a page`, `cmd shift a all`, `t tags`, `d delete`, `u unread · r read`, `h history`, `e export`, `: commands`. No all-caps, no categories, no letter-spacing.

### Authors, topics

A filter row at the top of the column: 720px wide, 36px tall, 1px `line` bottom border, placeholder `filter authors` / `filter topics` at the left, and the total data count at the right (10.5px mono). Pressing the row or `/` focuses the filter; typing filters the directory on the store, with an accent underline while editing and `esc` at the right. `esc` clears a non-empty filter before leaving the route. Rows: authors are 56px tall with a 2px selection rule, 32px avatar, a two-line name/handle block, and the post count right-aligned in 10.5px mono; topics are 48px tall with `#tag` and the count. Rows take a `band` hover fill. Real scroll viewport, 100 rows per page, the next page loads at the bottom, and a muted `loading` row appears while it arrives. Click filters the feed by author/tag and closes the route. Up/down moves, Enter filters, Esc returns to the feed. Empty filtered results show `no matches`. No cards, no grid.

### Graph

One of the two full-bleed screens (with the lightbox): the graph uses the whole content area after the 56px header and 24px footer, inset 24px, instead of the 720px reading column. Empty space is part of the instrument; nothing is cropped by a fixed square.

Nodes are tags: colored dots (drawn in one canvas batch, hit-tested against the same projection), labels as text; edges are hairline `line` strokes, thicker and accent-tinted around the focused node. The layout is deterministic force-directed on the unit disc (co-occurring tags attract, popular tags drift toward the centre, tag-name hash seeds the start), scaled to the field's aspect ratio, so a wide window fills its width. Wheel/pinch zoom around the pointer, Shift-wheel and drag pan, `+`/`-`/`0` (fit) and arrow keys; zoom and pan are clamped so the constellation can never be lost off-screen, and `0` returns to the fitted view. Double-press fits too.

`/` opens a tag filter: the field dims every node whose tag does not contain the text live, enter commits it (esc clears), and hover/click still work on dimmed dots. Adjacent labels skip anchors that would overlap an already-labelled one.

Label level of detail keeps a thousand tags legible: up to 140 tags show every label; larger graphs label the 36 most-saved tags plus whatever is focused, and the rest stay as dots. Click a tag to filter the feed; Esc returns. Hover/Tab focuses a node; `c` opens eight token color swatches and a persisted per-tag override. Enter filters. No counts on nodes; the count can appear in the status line while a node is focused.

### Calendar

Browses the archive by month and year without leaving the reading column.

- **Header:** `November 2026` in Newsreader 24, centred, with quiet `‹` / `›` chevrons. The title is a button that opens the **year overview**; returning from it restores the month. Clicking a chevron moves one month (or one year in the overview). Month switches fade and rise 160ms; reduced motion snaps.
- **Month grid:** mon-first, 5/6 weeks, 12.5px numbers, up to four 4px accent marks per saved day, a 2px accent rule on the selected week, and that week's saved-post count in 10.5px mono in the left gutter. Today's number is accent. Click a day to filter the feed by that day; click a mark to filter that day and open the post; click the week gutter/rule to select the week.
- **Year overview:** twelve month plates, four per row, 168x120 each. Each plate has a lowercase mono month name and a 7-column day grid of 4px dots: no saved post is `line`, one to two is 28% accent, three to eight is 55% accent, nine or more is full accent. The focused month carries the 2px accent rule; clicking a plate opens that month; clicking a day dot filters that day.
- **Keyboard (Calendar route):** `←`/`→` previous/next week (crossing a month edge loads the neighbour and continues), `↑`/`↓` previous/next month, `[`/`]` previous/next month, `y` toggles the year overview, `t` jumps to the current month, `Enter` filters the feed by the selected week, `Esc` leaves the year overview first, then returns to the feed. In the year overview arrows move the focused month (`←`/`→` by one, `↑`/`↓` by four), `[`/`]` move years, `Enter` opens the focused month.
- No dashboard chrome and no totals banner; the only count is the selected week's data count.

### Tag management

`rename tag` (palette) opens a two-step panel for the selected post: choose one of its tags, then type or pick the new name from autocomplete suggestions (existing tags with usage counts). Renaming into an existing tag merges the two, moving associations and the persisted colour; the document's `tags_text` is rebuilt so search follows. `prune tags` removes tags no bookmark uses (and their stale colours). The graph colour panel's `reset` deletes the stored override.

### Reading queue

`cmd-n` or the palette's `next unread` opens the next unread post after the current selection and marks the current post read; when the loaded view has no unread posts left it switches to the unread filter and starts from the newest unread. The `unread` route remains the full list; the queue is the one-key path through it.

### Notes

`n` or the palette's `edit note` opens a note editor for the selected post (420–620px panel, multi-line field, `save`/`clear`, cmd-enter saves, esc closes). A saved note renders under the post body in detail, is part of the FTS document (schema v2 column, v5 index) and round-trips through CSV/JSON exports. Notes are local-only and never sent anywhere.

### Import

The importer accepts CSV, JSON, archive `.js` and the X archive `.zip` directly. While a batch runs the route says which phase it is in (`reading files`, then `saving posts`) with file and row counts; `esc` cancels cooperatively before the write and the summary says nothing was written. Each file is reported separately: a file that cannot be parsed is listed with its error while the rest of the batch continues. The dry-run duplicate count uses the same conflict rules as the insert (archive URLs and ids, plus repeats inside the file and across the batch), so preview and commit agree.

Route, not a modal. 720px column. Source band with `drop files or choose them`, accepted formats `CSV, JSON or X archive` in 10.5px mono. Multi-file picker and drag/drop are real. Preview: every file with format, post count and duplicate count; `import N posts`. Importing: thin 2px accent progress, `cancel`. Done: `N posts imported` plus duplicates if true, `import another file`, and `back to feed`; the route never redirects on its own. Errors in place, one sentence. Escape cancels and returns. No marketing copy.

### Settings

Reachable by `,`, `Ctrl/Cmd-,`, palette, or the header's settings item. 720px column, grouped rows with 1px `line` separators only between groups:
- appearance: theme (system, light, dark), text size (regular, larger, largest)
- motion: system, reduced
- media: avatars, automatic/on-demand post media, image and video cache stats and clear
- motion: system or reduced
- library: database path (read-only), post count, author count, tag count
- about: version, source link

Settings persist in `settings.json`, not SQLite. No paragraphs explaining obvious controls.

### Onboarding

Empty archive only. One line `import your bookmarks export`, one drop zone, one file picker. Formats in 10.5px mono. No ASCII art, no feature tour.

### Window

System decorations on Linux. 1280x820 default, minimum 960x640. Remember size/position. Title `eterea`. No native menu. No custom titlebar.

## 5. Motion

- Only interaction feedback. Nothing animates on first frame.
- Selection rule: 120ms opacity, no slide.
- Hover: 120ms color/background.
- Detail: 160ms opacity + 4px rise.
- Search field/nav swap: 160ms opacity.
- Palette/key sheet: 140ms opacity, scale 0.995 to 1.
- Lightbox: 160ms scrim opacity, 180ms image fade.
- Media: 180ms opacity after load; no scale.
- Status line: 120ms fade.
- Reduced motion: all durations 0, except a 120ms opacity fade for overlays. No bounce, no springs, no parallax, no looping decoration.

## 6. Accessibility

- Contrast: body 4.5:1, large text 3:1, controls 4.5:1. Check token pairs in tests.
- Focus ring: 1px accent, 2px offset, visible on keyboard focus only.
- Pointer targets: at least 24x24px for text actions; 32px for nav rows; the full post row is the target for selection.
- Every icon-only control has a name. Every image has alt text or is marked decorative.
- Screen reader: Rune roles and labels for nav, post, author, media, favorite, quote, dialog, list, graph, calendar. The search field and directory filter expose a `searchbox` role and a name; the footer `keys` hint and the header `search` item expose a `button` role with a name; the key sheet exposes a `dialog` role with a name. The headless UI harness checks the accessibility tree on every route.
- Text scale from settings does not clip; layouts reflow at 125%.
- `prefers-reduced-motion` is honored.
- No keyboard trap. Text fields suppress globals, and Esc always returns.

## 7. Data and privacy

- Post fields: URL, text, posted date, imported date, author name/handle/avatar URL, tags, media list, quote, favorite, note, read state, URL entities.
- Quotes: author, text, posted date, media, optional unavailable flag. Missing quote body shows the quoted URL and `not in archive`; no invented text.
- Media: type, URL, best playback URL, preview/poster URL, alt, width, height, duration, variants. No media bytes in SQLite.
- Avatars load over HTTPS by default; a setting disables them and uses a monogram.
- Post media loads automatically by default; Settings can switch to on-demand. Media bytes live in the local cache, never SQLite. Video downloads cache only when played.
- No engagement counts, reach, verified badges, or synthetic fields.
- External links require HTTPS and an explicit action. No private/loopback hosts for media fetches.
- Remote images are requested without cookies or referrer. Disk cache is opt-in and bounded.

## 8. Copy

History is a route in the main reading column, not a modal: the five latest import, delete and tag actions newest-first, with a detail panel listing exactly the affected posts and before/after tags. Only the newest non-undone event can be undone; older rows explain why. Undo writes a status line and refreshes the archive. Export is a small sheet (scope: all / filter / selection; format: CSV / JSON), invoked from the palette, `e`, or History. Exports are not history events.

The copy deck is `docs/design/COPY.md` and is binding. Screen budgets: feed 0 chrome strings besides nav; detail 4; search 2; import 12; settings labels only; graph 2; calendar 2. No all-caps, no exclamation marks, no em dashes, no arrow suffixes, no filler.

Known deletions: `READING FEED`, `Showing 48 of 7,873 bookmarks`, `— ANALYTICS SCOPE —`, `SERVICE-DERIVED`, version in the shell, line/column status, `Reset View`/`Import` on every route, ASCII onboarding, stale keybinding copy.

## 9. Brand

- Header: 20px full-color translucent purple orb + `eterea` in Newsreader 20, on light and dark. The app icon and README use the primary orb; the Linux desktop entry carries the hicolor icon set.
- App icon: `docs/assets/brand/eterea-icon.svg` plus PNG 16/32/64/128/256/512.
- The wordmark is never set in all caps or with wide tracking.
- README: eza-style, centered mark and title, italic line, badges, light/dark hero, screenshot table, install, features, shortcuts. Written after real captures exist.

## 10. Acceptance

- Screens at 1440x900, 1280x820 and 960x640, light and dark: feed, selected feed, detail, quote, video playing, auto media, on-demand media, 20-item carousel, lightbox, search, palette, key sheet, authors, topics, graph with zoom/colors, calendar, import source/preview/importing/done/error, history list/detail/undo, export sheet, selection bar, settings, onboarding, empty, error.
- Keyboard: every key in section 3 works; the key sheet matches; no global key leaks in a field; Esc never traps.
- Mouse: selection, byline author filter, tag filter, media open, lightbox navigation, palette, import, settings.
- Data: no invented metrics; missing quote/media/alt states honest.
- Copy: `python3 scripts/copy-lint.py src` returns 0 errors.
- Motion: durations within section 5; reduced motion snaps; no animation on first frame.
- Accessibility: every route's accessibility tree passes the harness; contrast tokens pass.
- Performance and verification gates from the implementation plan.
