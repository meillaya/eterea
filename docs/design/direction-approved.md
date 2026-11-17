# Direction approved

Date: 2026-11-17

## What was shown

- `design-demos/dir-1.html`, Daylight. Screenshots: `design-demos/shots/dir-1.png`, `dir-1-detail.png`, `dir-1-elena.png`, `dir-1-graph.png`, `dir-1-calendar.png`.
- `design-demos/dir-2.html`, Signal. Screenshots: `design-demos/shots/dir-2.png`, `dir-2-post-elena.png`, `dir-2-palette.png`, `dir-2-graph.png`, `dir-2-calendar.png`.
- `design-demos/dir-3.html`, Screening. Screenshots: `design-demos/shots/dir-3.png`, `dir-3-grid.png`, `dir-3-text.png`, `dir-3-quote.png`.
- Six-candidate sheet: `design-demos/shots/thumbs.png`; cards in `design-demos/directions.md`.
- Brand sheet: `design-demos/shots/brand-sheet.png`; assets in `docs/assets/brand/`.

## User choice

> daylight but also with the keyboard first setup of signal

## Decision

- Visual system, layout, typography and surface treatment: **Daylight**.
- Interaction model, selection, command palette, key routing and status behavior: **Signal**.
- Kept as a product requirement: graph and calendar views, import, search, post detail, quotes, media, avatars, favorites.
- Kept as a product requirement: no invented engagement, no counts repeated in chrome, no all-caps labels, no filler copy.

## Consequences

- Build the Daylight single-column reading surface at 720px with a marginalia rail, in the Daylight tokens.
- Use Signal's keyboard-first behavior inside that surface: `j`/`k` selection, `Enter` open, `Esc` back, `/` search, `:` command palette, `?` keys, `f` favorite, `c` copy link, `o` open on X, hidden-media load.
- A slim status line appears only while it has state; no permanent shortcut hints.
- The header uses the mono eterea mark and wordmark; the vermilion tab stays brand-only so the UI keeps one link/focus accent.

## Not changed

- The logo mark and the eza-style README requirement remain part of the build. The README is recast after real light and dark captures exist.
