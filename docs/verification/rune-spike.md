# Rune Linux spike

Date: 2026-11-17. Rune revision `842c28c2ca5dd1506c1e4a67052db61800aed721`.

## What was checked

- A standalone crate with `rune` at the pinned revision, the five crates from Rune's
  `[patch.crates-io]` table, and a 480x320 window.
- A JPEG loaded through `ImageCache`; text with Japanese, Arabic, and an emoji.
- A release-free debug build on Rust 1.99.1 with the system Wayland/X11 libraries.
- A window run for 8 seconds with no crash (only dconf warnings because the sandbox
  cannot write `/run/user/1000/dconf`).
- A headless render: `rune-spike --snap spike.png --tree spike.json`.
- The snapshot shows the JPEG avatar, the emoji in color, Japanese, and shaped Arabic.

## Environment notes

- Headless rendering needs a Vulkan adapter. The agent shell has no `/dev/dri`, so
  the render used Mesa's lavapipe from the Nix store:
  `VK_ICD_FILENAMES=<mesa>/share/vulkan/icd.d/lvp_icd.x86_64.json` and
  `LD_LIBRARY_PATH=<mesa>/lib`.
- A desktop session with RADV uses the normal adapter; no override is needed.
- Debug target size for this one small app: 3.3 GB. The release build and mbx
  managed targets are required before this is shippable.
- The debug binary is 596 MB because of debug info. Release sizes are measured
  after the app exists.

## Consequence

Rune is viable for the rewrite on this machine. CI needs a software Vulkan driver
for headless UI snapshots; local runs use the desktop GPU.
