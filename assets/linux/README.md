# Linux bundle assets

Static files the release packager (`scripts/package-linux-release.sh`) installs
into the Linux bundle's `share/` tree:

| File | Installed as |
| --- | --- |
| `eterea.desktop` | `share/applications/eterea.desktop` |
| `../docs/assets/brand/icon-<N>.png` | `share/icons/hicolor/<N>x<N>/apps/eterea.png` (every `icon-<N>.png` in the brand directory: 16, 32, 64, 128, 256, 512, 1024 today) |
| `../docs/assets/brand/eterea-icon.svg` | `share/icons/hicolor/scalable/apps/eterea.svg` |

The brand assets are authored under `docs/assets/brand/` (owned by the asset
task, not duplicated here). The packager copies them when present and prints a
warning when a size is missing, so the bundle can be built before the final
icons land.

## Installed layout

```text
share/applications/eterea.desktop
share/icons/hicolor/16x16/apps/eterea.png
share/icons/hicolor/32x32/apps/eterea.png
share/icons/hicolor/64x64/apps/eterea.png
share/icons/hicolor/128x128/apps/eterea.png
share/icons/hicolor/256x256/apps/eterea.png
share/icons/hicolor/512x512/apps/eterea.png
share/icons/hicolor/1024x1024/apps/eterea.png
share/icons/hicolor/scalable/apps/eterea.svg
```

The desktop entry uses `Exec=eterea` and `Icon=eterea`, which is correct once
`eterea` is on `PATH` and the icon theme caches are updated (`gtk-update-icon-cache`
or the first desktop login after install). Running the bundle in place works
with `./eterea` (direct) or `./eterea-launcher` (portable loader path).

## Installing the bundle into a prefix

```sh
prefix="${HOME}/.local"
install -Dm755 eterea          "${prefix}/bin/eterea"
cp -r lib                      "${prefix}/lib/eterea"
install -Dm644 share/applications/eterea.desktop \
  "${prefix}/share/applications/eterea.desktop"
cp -r share/icons              "${prefix}/share/"
```

Note that `share` is additive; copying `icons` into an existing
`${prefix}/share/icons` merges the hicolor theme as expected.
