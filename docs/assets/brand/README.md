# eterea brand

The mark is a translucent light-purple orb: a warm-white lavender core, a soft violet rim, a broad low-opacity glow, and one restrained highlight arc. It is light made visible, not a letter, a bookmark, or a badge. There is no hard outline and no second symbol.

## Files

| File | Use |
| --- | --- |
| `eterea-orb.svg` | Primary orb for light surfaces. Transparent, 1024x1024 canvas. |
| `eterea-orb-light.svg` | Orb tuned for dark surfaces. Transparent, 1024x1024 canvas. |
| `eterea-orb-header.svg` | Full-color header orb, 20x20 canvas and 20px minimum. Use it at 20px in the app header on both light and dark surfaces. |
| `eterea-orb-mono.svg` | One-color fallback mark, 20x20 canvas. Uses `currentColor`; set it to the surface ink (ink `#1a1a18` on light, `#ecebe4` on dark). Use only where full color is impossible. |
| `eterea-icon.svg` | App icon master. Transparent, 1024x1024 canvas. |
| `icon-{16,32,64,128,256,512,1024}.png` | App icon PNGs. RGBA, transparent, one size per file. |

The earlier lowercase-e and bookmark-tab mark is retired and removed. Do not reintroduce a lettered or bookmark-tab mark.

## Usage

- Header: `eterea-orb-header.svg` at 20px plus the wordmark `eterea` in Newsreader 400 at 20px, normal tracking. Use it on both light and dark headers. The mono mark is the fallback where full color is not possible.
- README and release surfaces: `eterea-orb.svg` on light backgrounds, `eterea-orb-light.svg` on dark backgrounds.
- App icon: use `eterea-icon.svg` or the matching `icon-N.png`. Keep the transparent background; a platform tile may sit behind it, never bake a new background into the assets.
- The orb keeps its transparency. Place it on `#f7f6f3` (light), `#141412` (dark), plain white, or plain near-black. Do not put it on busy photography or on a mid-tone that swallows the rim.
- Clear space: at least one quarter of the visible orb diameter on every side. The SVG canvas already includes the outer glow as part of that space; do not crop into it.
- Minimum size: 16px for the app icon PNGs, 20px for the header orb, and 20px for the mono fallback. Do not use the full-color orb below 32px outside the header and app icon sets.
- The orb is brand-only. Do not use its violet as an app-chrome accent; the product accent stays `#2447b8`.

## Color

| Token | Value |
| --- | --- |
| Orb violet (brand) | `#8f6fc8` |
| Deep violet (rim) | `#7d5cbb` |
| Lavender (body) | `#ecdcf7` |
| Warm white (core) | `#fdf6f1` |
| Light-surface ink | `#1a1a18` |
| Dark-surface ink | `#ecebe4` |
| Light surface | `#f7f6f3` |
| Dark surface | `#141412` |

## Do not

- Do not add lettering, a bookmark tab, a badge, a drop shadow, a hard stroke, or a second glow.
- Do not add extra rings, sparkles, stars, or light streaks; one highlight arc is the whole gesture.
- Do not rotate, recolor, squash, upscale from a small PNG, or redraw the gradients.
- Do not set the wordmark in capitals or with wide tracking.
- Do not place the orb inside a second circular badge or tile in the master files.
- Do not add the large halo to `eterea-orb-header.svg`; its silhouette has to hold at 20px.
- Do not use the retired e-and-tab mark.

## Regenerating the PNGs

Render from `eterea-icon.svg` and downscale so every size comes from the same master:

```sh
magick -background none eterea-icon.svg -resize 1024x1024 -depth 8 -strip \
  -define png:color-type=6 -define png:compression-level=9 icon-1024.png
for n in 512 256 128 64 32 16; do
  magick icon-1024.png -filter Lanczos -resize ${n}x${n} -depth 8 -strip \
    -define png:color-type=6 -define png:compression-level=9 icon-$n.png
done
```

Every PNG must stay RGBA with transparent corners and no baked background.
