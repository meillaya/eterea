# Linux release packaging

How eterea turns the devenv-built desktop binary into a runnable Linux bundle,
and how CI verifies it. Scope: `scripts/package-linux-release.sh`,
`scripts/smoke-linux-bundle.sh`, `assets/linux/`, the Nix package install step
and `.github/workflows/draft-release.yml`.

## The problem this fixes

`target/release/eterea` built in the devenv shell only runs inside that shell.
winit `dlopen`s Wayland/X11/xkbcommon/fontconfig libraries by soname and wgpu
`dlopen`s Vulkan, so a launch from a file manager (no `LD_LIBRARY_PATH`) fails:

```text
event loop: os error at .../winit/src/platform_impl/linux/wayland/event_loop/mod.rs:89:
The wayland library could not be loaded
```

The release binary also carries a RUNPATH pointing at `/nix/store/...` paths
from the build environment, which does not exist on another machine.

## Usage

Run inside the devenv shell (toolchain + library closure come from there):

```sh
# full release build + bundle
devenv shell -- bash scripts/package-linux-release.sh

# mechanics-only: bundle the existing binary (refused when stale unless you say
# --allow-stale; never use this for a release artifact)
devenv shell -- bash scripts/package-linux-release.sh --skip-build --allow-stale
```

Output:

```text
dist/
  eterea-<version>-linux-x86_64/
    eterea                  ELF, RUNPATH=$ORIGIN/lib (direct launch)
    eterea-launcher         POSIX sh, runs eterea through lib/ld-linux-...so.2
    lib/                    collected runtime libraries (62 in the 2026-11-17 bundle)
    share/applications/eterea.desktop
    share/icons/hicolor/... 16/32/64/128/256/512 png + scalable svg
    PROVENANCE.md
    SHA256SUMS
  eterea-<version>-linux-x86_64.tar.gz
  eterea-<version>-linux-x86_64.tar.gz.sha256
```

Observed 2026-11-17 final bundle (rebuilt from the green tooling-enabled
release build at 18:07; source tree dirty during integration):
bundle 37 MB, tarball 18 MB, 51 shared libraries, 64 files, 63 checksummed files
(SHA256SUMS itself excluded). 7 PNG hicolor sizes (16-1024) + the scalable SVG.
The binary and bundled libraries require glibc <= 2.39 symbols, so Ubuntu 24.04
(glibc 2.39) and newer hosts run it; the ELF interpreter is
`/lib64/ld-linux-x86-64.so.2`.

## What the packager collects (and what it deliberately does not)

1. The binary's non-glibc NEEDED closure (recursively).
2. Libraries winit/fontconfig `dlopen` by soname (they never appear in `ldd` of
   the binary): Wayland client/cursor/egl, xkbcommon(+x11), the X11 stack (X11,
   X11-xcb, Xcursor, Xrandr, Xi, Xinerama), fontconfig, freetype, expat, zlib
   and the libxcb extension sonames present in the closure.
3. **Not** glibc and **not** the dynamic loader by default. The bundle uses the
   host glibc, and the ELF interpreter is rewritten to
   `/lib64/ld-linux-x86-64.so.2`. This is the important fix: a bundled glibc
   cannot load host GPU drivers built against a newer glibc (see the incident
   below). `--bundle-glibc` opts back into shipping the build loader + glibc.
4. **Not** libvulkan/libGL/libEGL by default; the host desktop's GPU stack is
   used so Mesa/ICD and glibc always match. `--bundle-gpu` bundles them.

The launcher prefers a host dynamic loader (`/lib64`, `/lib`, `/usr/lib`,
`/usr/lib64`); if none exists it falls back to a bundled loader when
`--bundle-glibc` was used, and otherwise fails with a clear message.

Search paths are, in order: `LD_LIBRARY_PATH` from the devenv shell, then every
`lib/` directory of the resolved `.devenv/profile` closure (`nix-store -qR`).
That is how the X11 stack is found even though the dev shell only puts Wayland
libraries on `LD_LIBRARY_PATH`.

Then:

* `eterea` gets `RUNPATH=$ORIGIN/lib` (patchelf); every bundled library gets
  `RUNPATH=$ORIGIN` so transitive dependencies resolve inside `lib/`.
* `eterea-launcher` exists because an ELF interpreter path cannot be made
  relative: it execs the bundled loader with `--library-path lib`, which makes
  the bundle runnable on hosts without the build environment's `/nix/store`
  glibc. The desktop entry still points at `eterea` (direct, RPATH fixed).
* The packager refuses to package a `--skip-build` binary older than the desktop
  sources. A failed build is a hard error; it never falls back to an old binary.
* `PROVENANCE.md` contains name/version/target/source commit/build command/date
  and library provenance, with no personal paths or URLs. `SHA256SUMS` hashes
  every other file in the bundle.

## Incident 2026-11-17: "could not create the window ... Failed to create surface for any enabled backend"

The first portable bundle shipped glibc 2.40 and the Nix loader. On the
development host the packaged app (and its launcher) failed under
Xvfb+lavapipe with:

```text
rune: could not create the window main: Failed to create surface for any enabled backend: {}
```

Root cause: the session's Mesa 26.2.3 lavapipe ICD loads `libLLVM.so.21.1`,
which needs `GLIBC_ABI_GNU2_TLS` from glibc 2.42+. The bundle's own
`libc.so.6` (2.40) shadowed the host glibc, so the Vulkan loader rejected the
ICD ("Found no drivers!", visible with `VK_LOADER_DEBUG=error,warn`), wgpu's
Vulkan instance had zero drivers, and surface creation failed before any X11 or
Wayland issue could matter. The app running under the dev shell had been smoked
against the devshell's Mesa 25.3.0, which matches glibc 2.40 — that is why it
"worked" while the packaged bundle did not.

Verified fix and evidence (session Mesa 26.2.3 ICD, forced Xvfb, no Wayland):

| Variant | Direct ELF | Launcher |
| --- | --- | --- |
| old bundle (bundled glibc 2.40 + loader) | surface error | surface error |
| new bundle (host glibc, bundled Vulkan loader) | **PASS 3 s** | **PASS 3 s** |
| new bundle (host glibc, host Vulkan/GL) | **PASS 3 s** | **PASS 3 s** |
| new bundle direct, `WINIT_UNIX_BACKEND=x11` | **PASS 3 s** | n/a |

Minimum shared-library set needed from the bundle to create a window: Wayland
client/cursor/egl, xkbcommon(+x11), X11/X11-xcb/Xcursor/Xrandr/Xi/Xinerama,
fontconfig, freetype, expat, zlib and the xcb extension sonames. The GPU stack
(Vulkan loader, ICD/driver, GL/EGL) can and by default should come from the
host. `--bundle-gpu` is available but must match the host glibc.

## Smoke test

`scripts/smoke-linux-bundle.sh` has two modes. It launches from a clean-ish
environment (`LD_LIBRARY_PATH`, `LD_PRELOAD`, `WAYLAND_*` unset) and re-execs
itself under `xvfb-run` when no `DISPLAY` is set.

* `--snap` (stronger, used by CI): runs the packaged app headlessly with Rune's tooling
  (`--snap out.png --tree out.json --at 1`), requires a clean exit and a
  non-empty PNG/tree. This proves the bundle created a window, a GPU surface and
  rendered a real frame. Requires a release build with Rune's `tooling` feature
  (enabled in `src/desktop/Cargo.toml`).
* default (process mode): starts the app for N seconds and fails if it exits or
  reports a window/library error.

```sh
# direct ELF (what the desktop entry runs), real frame
scripts/smoke-linux-bundle.sh dist/eterea-*-linux-x86_64 --snap --at 1 \
  --icd /usr/share/vulkan/icd.d/lvp_icd.x86_64.json

# portable launcher (host loader + bundle libs), real frame
scripts/smoke-linux-bundle.sh dist/eterea-*-linux-x86_64 --launcher --snap --at 1 \
  --icd /usr/share/vulkan/icd.d/lvp_icd.x86_64.json

# process-alive check
scripts/smoke-linux-bundle.sh dist/eterea-*-linux-x86_64 --seconds 3 \
  --icd /usr/share/vulkan/icd.d/lvp_icd.x86_64.json
```

Final bundle results on the development host (NixOS, no `/dev/dri` for the
agent shell), always under a **fresh** `xvfb-run -a` with `WAYLAND_DISPLAY`
unset and the session's Mesa 26.2.3 lavapipe ICD:

| Check | Direct ELF | Portable launcher |
| --- | --- | --- |
| `--snap --tree --at 1` | **PASS**: 399,402-byte PNG (2560x1640), 504,710-byte accessibility tree | **PASS**: same PNG/tree |
| process alive 3 s | **PASS** | **PASS** |
The app logs harmless `Fontconfig warning: /etc/fonts/...` lines because the
bundled fontconfig reads the host font configuration; it does not fail on them.

The old failure is reproducible by running the raw release binary without the
bundle:

```sh
timeout 8 ./target/release/eterea; echo "exit=$?"   # exit=1, wayland load error
```

## CI (`.github/workflows/draft-release.yml`)

`release` job (tag `v*`):
1. build `devenv shell -- cargo build -p eterea-desktop --release`;
2. `devenv shell -- bash scripts/package-linux-release.sh --skip-build`;
3. install Xvfb + Mesa lavapipe via apt, verify bundle contents and `SHA256SUMS`,
   then smoke the **direct ELF** for 3 s;
4. assemble release artifacts (bundle tarball + its sha256 + PROVENANCE.md +
   SHA256SUMS), upload the artifact, create the draft GitHub release.

`smoke` job (`needs: release`): downloads the uploaded artifact, extracts it,
and smoke-tests the **portable launcher** under Xvfb + lavapipe, i.e. exactly
the bytes a user downloads, without any Nix environment.

## Known limits

* Bundle is Linux x86_64 only, built with Rust 1.99.1 from the pinned devenv.
* The direct ELF keeps the build environment's ELF interpreter path; it runs on
  the build host/NixOS and in the release job. On other distributions use
  `eterea-launcher` (bundled loader+glibc), which is what the uploaded-artifact
  smoke job verifies.
* Bundling glibc + loader is ~2.5 MB of the archive and is the standard trade-off
  for running outside the build environment; signing/notarization is not done.
* Mesa lavapipe is only used for the CI/headless smoke. A desktop session uses
  its normal GPU driver; there is no GPU in the CI runner.
* No `.desktop` validation tool (`desktop-file-validate`) or `shellcheck` was
  available on the reference host; the scripts were syntax-checked with
  `bash -n` and the workflow with the repository validators + actionlint.
* The reference build was produced from a dirty working tree during app
  integration; `PROVENANCE.md` records `Source tree: dirty (...)`. A release
  build from a clean checkout records the real commit.
* The CI smoke path (GitHub runner, apt Mesa lavapipe) has not been executed on
  GitHub; it is the same script and commands verified locally against the
  Nix-packaged Mesa equivalent.
* `dist/` is generated output; it is not committed and should be added to the
  repository `.gitignore` by the owner of the root file.
