# Copy deck

User-facing strings are part of the design. Every string has a job: data, an action, or navigation. If it does none of those, delete it.

## Voice

- Plain, specific, lowercase. Product nouns: eterea, post, author, topic, archive, import, graph, calendar.
- A label names the thing or the action. It does not sell the thing.
- One fact in one place. Do not repeat a handle, date, count, or state in a second region.
- No exclamation marks. No em dashes. No arrows at the end of labels. No "…" unless text is actually truncated.
- Errors: what failed, then the next action. One sentence. No apology.
- Empty states: one sentence, one action.

## Banned

seamless, effortless, delightful, elevate, elevated, unlock, empower, supercharge, reimagined, curated, archive-first, service-derived, ai-powered, cutting-edge, state-of-the-art, game-changer, revolutionary, next-level, blazing, lightning-fast, beautifully, intuitive, bespoke, tailored, immerse, immersive, dive in, welcome to, get started, learn more, discover, explore, your library, showing/loaded counts.

## Screen budgets

| Screen | Chrome strings (excludes post data) |
| --- | --- |
| Feed | 8 |
| Detail | 6 |
| Search | 5 |
| Import | 12 |
| Settings | labels only |
| Graph | 6 |
| Calendar | 6 |

Chrome means navigation, controls, state, and headings. Post text, author names, handles, dates, tag names, and media alt text are data, not chrome.

## Known deletions from the current app

- `READING FEED` eyebrow, repeated on every route.
- `Showing 48 of 7,873 bookmarks • 5,204 authors in the archive.` The feed shows one count once.
- `— ANALYTICS SCOPE —` and `SERVICE-DERIVED` stamps.
- Version string in the shell.
- `0/7873 · MOCHA · LN 1 COL 1` status noise.
- `Reset View` and `Import` repeated on every route.
- ASCII onboarding art.
- Stale keybinding copy; the sheet is generated from the keymap.
- Per-post "Remote images hidden" paragraphs; one action in the media slot.

## Enforcement

`scripts/copy-lint.py` checks HTML text and Rust string literals for banned phrases, all-caps labels, exclamation marks, em dashes, arrow suffixes, and repeated text. CI runs it on the app and the design prototypes. Exceptions need an inline `copy-ok: reason` and review.
