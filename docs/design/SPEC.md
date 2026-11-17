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
- Header: 56px tall, 52px side padding. 20px translucent purple orb + wordmark 20px at the left; nav 12.5px at the right with 26px gaps: feed, authors, topics, graph, calendar, import, history, settings.
- Feed: reading column 720px centered. Marginalia rail 150px wide, right-aligned, ending 30px left of the column. Content starts 64px below the header.
- Post rhythm: 64px between posts. One 1px `line` only when the saved day changes, spanning the reading column.
- Selection: 2px `ink` rule at 14px left of the reading column, full post height. Selected post scrolls into view.
- Detail: same column, 740px when media needs the room, vertically centered when short. Back returns to the previous scroll position. Viewer tags are editable, the last line carries `edit tags` and `delete`; delete confirms before writing.
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

- `Esc` never traps. It closes the top overlay, then search, then leaves detail.
- Text fields swallow keys. The palette and search are the only text contexts outside import and settings.
- No global shortcut uses Ctrl or Cmd except `Ctrl/Cmd-P` for the palette, `Ctrl/Cmd-F` for search, `Ctrl/Cmd-,` for settings, and clipboard/quit shortcuts reserved by the OS.
- The key sheet is generated from the same table the handler uses. It cannot drift.

### Selection

- The feed always has a selection once it has posts. The first post is selected on load.
- `j`/`k` moves and scrolls the selected post into view.
- Clicking a post selects it; clicking the byline opens the author filter; clicking the post body opens detail.
- The favorite marker is a 6px accent square in the marginalia, shown only when favorited.
- The rail actions appear on hover or focus; they are text, never icons.

### Status

- 24px bottom area, 11.5px text, 52px left inset. It exists only while it has a message: `link copied`, an import result, or an error. It clears after 2s or on the next action.
- No permanent shortcut hints, mode badges, counts, line/column numbers, theme names, or version strings.

## 4. Screens and components

### Feed

Post: rail (date, time, favorite pin, actions), byline (30px avatar, name, handle), text, media, quote. No card, no border, no shadow. Links are accent text underlined on hover. Hashtags and mentions are links to filtered views. Emoji render in color. CJK and RTL follow the font fallback.

Media: avatars, images and video posters load automatically by default, fading in 180ms; Settings can switch post media to on-demand and keep the 720x180 `band` with `load media` for that mode. Images use the best available rendition through a bounded disk cache. A post with up to 20 media renders a paged carousel (1/2/3/4 layouts; >4 pages in groups of four with `‹ n/m ›`); the lightbox covers every item. X and quote URLs are removed from the text; external links keep their display URL; quote cards use the same media rules.

Video: poster fills the media slot, centered 44px hairline play circle, 10.5px mono duration at bottom right. Playback resolves the best progressive MP4 variant, caches the file persistently, and uses `ffmpeg` for frames. Inline playback is muted and looping in the feed. Detail and lightbox carry sound; `m` toggles it, and `ffplay` supplies audio from the cached local file. Missing ffmpeg/ffplay is a status error, not silence.

Images: click or Enter opens the lightbox. The lightbox is `scrim` with the image centered, close at top right, previous/next at the sides; arrows on hover; Esc closes. No captions except the mono count.

Empty feed: one line `nothing here yet` in Newsreader 24, one action `import a bookmarks export`. No ASCII, no illustration.

No matches: `no matches`. One line, muted.

Errors: one sentence in the status area, then the next action, e.g. `this file is not a bookmarks export. choose a CSV, JSON or X archive.`

### Detail

Same column; 740px when a gallery needs it. Post, media at full size, quote, editable tags, metadata in 10.5px mono at the bottom (`saved 24 aug 2025`, source). Actions: favorite, copy link, open on X, edit tags, delete, load media when on demand. Back with `Esc`, `Backspace`, or the wordmark. `j`/`k` move to the next/previous post without leaving detail. Opening marks the post read.

### Header search

`/` or `Ctrl/Cmd-F` reveals a 360px field at the right, replacing the nav with a 160ms fade. Placeholder `search`. Typing filters the feed over text, name, handle, tags. Enter opens the first result. Esc clears and closes. No result count, no "results" label. A palette command `unread` filters unread posts.

### Command palette

`:` or `Ctrl/Cmd-P`. 560px wide, top 80px, centered, `bg` panel with a 1px `line` border, 2px radius. One field, one list. Commands, lowercase, no descriptions, keycaps only where a shortcut exists: feed, favorites, authors, topics, graph, calendar, import, settings, favorite, load media, copy link, open on x, clear filter, reload. Filter with substring; arrows move; Enter runs; Esc closes. No footer hints.

### Key sheet

`?`. Right-aligned 300px panel, 1px `line`, 2px radius. Rows generated from the keymap: `j k move`, `enter open`, `esc close`, `/ search`, `: commands`, `? keys`, `f favorite`, `c copy link`, `o open on x`, `m load media`, `← → media`, `space play`. No all-caps, no categories, no letter-spacing.

### Authors, topics

Paged list at the reading column: avatar 24, name 13, handle 12, count only if it is a data count (10.5px mono, right). Real scroll viewport, 100 rows per page, next page loaded at the bottom. Click filters the feed by author/tag and closes the route. Up/down moves, Enter filters, Esc returns. `/` filters the directory. No cards, no grid.

### Graph

White field. Nodes are tags, 12.5px labels, colored dots; edges are 1px `line`. Deterministic layout, 720px square centered, with wheel/pinch zoom around the pointer, Shift-wheel and drag pan, `+`/`-`/`0` and arrow keys. Click a tag to filter the feed; Esc returns. Hover/Tab focuses a node; `c` opens eight token color swatches and a persisted per-tag override. Enter filters. No counts on nodes; the count can appear in the status line while a node is focused.

### Calendar

Month title in Newsreader 24. Mon-first 5-week grid, 12.5px numbers, 4px marks per saved post, 2px accent rule on the selected week. Click a week to filter/scroll the feed; click a mark to open the post. Arrow keys move weeks; Enter filters. No dashboard chrome, no totals banner. One count in the selected week row if it is a data count.

### Import

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
- Screen reader: Rune roles and labels for nav, post, author, media, favorite, quote, dialog, list, graph, calendar. The headless UI harness checks the accessibility tree on every route.
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
