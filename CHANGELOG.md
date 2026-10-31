# Changelog

All notable changes to Eterea are recorded here. This project follows a
human-readable changelog and keeps release notes honest about the evidence that
has actually been captured.

## Unreleased

- Added a rendered desktop architecture figure and rewrote the README around
  the current import, library, and service boundary.
- Switched the development shell to `devenv.nix` and removed the duplicate
  flake dev shell and `shell.nix`.
- Reduced the release binary and import path: narrowed Dioxus features, removed
  unused regex and tokio dependencies, cached tag IDs, and made imports run in
  one transaction.
- Keep release notes aligned with the local-first privacy model documented in
  [release readiness](docs/operations/release-readiness.md).

## 0.1.0 production-candidate

Initial production-candidate desktop release surface.

- Local-first X/Twitter bookmark archive with CSV, JSON, and X archive import
  paths.
- Dry-run import preview before committed writes.
- SQLite-backed library, favorites, search, author/topic directories, detail
  routes, and table, tree, dashboard, graph, and calendar views.
- Dioxus desktop shell with session-only appearance settings and remote media
  previews hidden by default.
- devenv-based verification path documented in [development](docs/development.md).

Known release follow-ups remain tracked in
[release readiness](docs/operations/release-readiness.md), including live desktop
screenshot/timing, installer/signing, and release-grade performance evidence.
