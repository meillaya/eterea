# Development

## Environment

Use the devenv shell as the canonical environment:

```bash
devenv shell
```

With direnv installed, entering the repository activates the same shell through
`.envrc`.

The shell provides the native libraries needed by the Rust workspace. Running
plain `cargo test --workspace` outside the shell can fail on hosts without
OpenSSL pkg-config metadata.

## Common commands

```bash
# Run the desktop app
devenv shell -- cargo run -p eterea-dioxus

# Optional developer/QA fullscreen smoke check
ETEREA_WINDOW_MODE=fullscreen devenv shell -- cargo run -p eterea-dioxus

# Fast compile check
devenv shell -- cargo check --workspace

# Formatting, linting, and tests
devenv shell -- cargo fmt --all -- --check
devenv shell -- cargo clippy --workspace --all-targets -- -D warnings
devenv shell -- cargo test --workspace

# Release build smoke check
devenv shell -- cargo build -p eterea-dioxus --release
```

## Change hygiene

- Prefer small, reversible changes.
- Add or update tests before changing behavior.
- Do not add dependencies without a clear reason and review.
- Keep `Cargo.lock` committed because this workspace ships binaries.
- Keep `.omx/`, `.omc/`, local databases, and generated standalone exports out
  of version control.

## Manual smoke checks

Before a public release, run the desktop app and verify:

- the default desktop window is decorated, resizable, 1280x820, and has no
  native app menu where the platform/Dioxus host supports menu removal
- web content reaches the webview client-area edges inside the window frame
- `ETEREA_WINDOW_MODE=fullscreen` (or its `kiosk` alias) enters the explicit
  fullscreen path and can be exited/restored with the platform window controls
  or window-manager shortcut
- first launch and empty/onboarding state
- import dry-run and committed import paths
- search, author/topic filters, favorites, deletion, and detail routes
- all library views
- remote media preview opt-in/off behavior

## Live desktop QA harness

Real release desktop QA must launch the Dioxus desktop app, not a unit test or
web-only surrogate. Use the harness below from a graphical desktop session:

```bash
scripts/desktop-qa.sh
```

The script starts the app in tmux with the real command
`tmux new-session -d -s eterea-desktop-qa 'cd "$PWD" && devenv shell -- cargo run -p eterea-dioxus'`,
captures live logs with
`tmux capture-pane -pt eterea-desktop-qa -S -2000 > .omo/evidence/production-grade-project/desktop-tmux.log`,
records launch timing for the first usable desktop shell in
`.omo/evidence/production-grade-project/desktop-first-usable.txt`, and captures
a desktop screenshot with `gnome-screenshot -f .omo/evidence/production-grade-project/live-desktop.png`
or `import -window root .omo/evidence/production-grade-project/live-desktop.png`.

If neither `DISPLAY` nor `WAYLAND_DISPLAY` is set, the result is
`BLOCKED: live desktop screenshot unavailable`. The harness writes
`.omo/evidence/production-grade-project/desktop-display-blocker.md`; that
no-display blocker is not a pass and must keep production approval open until a
live desktop screenshot and first usable timing are captured on a real display.

## Desktop window modes

The Dioxus shell launches with an explicit desktop window configuration instead
of relying on framework defaults. The default mode is a decorated, resizable
1280x820 window with the native menu disabled where supported.

Fullscreen is deliberately opt-in and intended as a developer/QA smoke-check
surface, not a packaged user preference:

```bash
ETEREA_WINDOW_MODE=fullscreen devenv shell -- cargo run -p eterea-dioxus
```

The `kiosk` value is accepted as a fullscreen smoke-check alias for test
scripts. Both values use borderless fullscreen on the current monitor. This is a
manual smoke-check surface, not the default release mode; verify exit/restore
behavior on every target OS/window manager before presenting it as supported.

## Screenshot and state overrides

Docs screenshots and UI QA can start on a specific route, layout, and window
position. These overrides apply once at startup and do not persist:

```bash
ETEREA_INITIAL_ROUTE=library \
ETEREA_INITIAL_LAYOUT=dashboard \
ETEREA_WINDOW_X=40 \
ETEREA_WINDOW_Y=40 \
devenv shell -- cargo run -p eterea-dioxus
```

`ETEREA_INITIAL_ROUTE` accepts the route ids parsed by the shell: `library`,
`authors`, `topics`, `search`, `import`, `settings`, and `onboarding`.
`ETEREA_INITIAL_LAYOUT` accepts `table`, `tree`, `dashboard`, `graph`, and
`calendar`. Inside the shell, `Enter` opens detail for the highlighted entry.
