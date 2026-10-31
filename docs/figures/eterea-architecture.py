#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Generate the Eterea desktop architecture scene.

Run `python3 -B docs/figures/eterea-architecture.py` from the repository root,
then render the scene with the excalidraw CLI:

    excalidraw render docs/figures/eterea-architecture.excalidraw \\
      --out docs/figures/eterea-architecture.png --padding 40 \\
      --scale 2 --max-width 3200 --max-height 2000

On macOS, set `EXCALIDRAW_BROWSER_PATH` to a Chrome or Chromium binary. The
scene is deterministic: the seeds and timestamp are fixed, so a rerun produces
the same JSON.
"""

import json
from pathlib import Path

SCENE_PATH = Path(__file__).with_suffix(".excalidraw")
UPDATED_MS = 1_760_000_000_000

# Palette roles from the drawing rules.
INK = "#1e1e1e"
MUTED = "#5c5f66"
ARROW = "#495057"
INPUT = {"stroke": "#1971c2", "fill": "#d0ebff"}
RUNTIME = {"stroke": "#9c36b5", "fill": "#f3d9fa"}
STORE = {"stroke": "#f08c00", "fill": "#fff3bf"}

BODY_FONT = 22
NOTE_FONT = 18
TITLE_FONT = 32

_seed = 1000


def next_seed() -> int:
    """Return a stable seed for the next element."""
    global _seed
    _seed += 1
    return _seed


def base(
    element_id: str,
    element_type: str,
    x: float,
    y: float,
    width: float,
    height: float,
    stroke: str,
    background: str,
) -> dict:
    """Return the shared Excalidraw fields for one element."""
    seed = next_seed()

    if element_type == "rectangle":
        roundness = {"type": 3}
    elif element_type == "arrow":
        roundness = {"type": 2}
    else:
        roundness = None

    return {
        "id": element_id,
        "type": element_type,
        "x": x,
        "y": y,
        "width": width,
        "height": height,
        "angle": 0,
        "strokeColor": stroke,
        "backgroundColor": background,
        "fillStyle": "hachure",
        "strokeWidth": 2,
        "strokeStyle": "solid",
        "roughness": 2,
        "opacity": 100,
        "groupIds": [],
        "frameId": None,
        "roundness": roundness,
        "seed": seed,
        "version": 1,
        "versionNonce": seed,
        "isDeleted": False,
        "boundElements": [],
        "updated": UPDATED_MS,
        "link": None,
        "locked": False,
    }


def box(
    element_id: str,
    x: float,
    y: float,
    width: float,
    height: float,
    role: dict,
    label: str,
) -> list[dict]:
    """Return a rectangle and its bound label."""
    text_id = f"{element_id}-label"
    rectangle = base(element_id, "rectangle", x, y, width, height, role["stroke"], role["fill"])
    rectangle["boundElements"] = [{"type": "text", "id": text_id}]

    text_height = BODY_FONT * 1.25
    text = base(
        text_id,
        "text",
        x + 12,
        y + (height - text_height) / 2,
        width - 24,
        text_height,
        role["stroke"],
        "transparent",
    )
    text.update(
        {
            "text": label,
            "fontSize": BODY_FONT,
            "fontFamily": 1,
            "textAlign": "center",
            "verticalAlign": "middle",
            "containerId": element_id,
            "originalText": label,
            "lineHeight": 1.25,
        }
    )
    return [rectangle, text]


def label(
    element_id: str,
    x: float,
    y: float,
    width: float,
    text: str,
    color: str = ARROW,
    font_size: int = NOTE_FONT,
    align: str = "center",
) -> dict:
    """Return a free text label, used for arrow captions and notes."""
    height = font_size * 1.25
    element = base(element_id, "text", x, y, width, height, color, "transparent")
    element.update(
        {
            "text": text,
            "fontSize": font_size,
            "fontFamily": 1,
            "textAlign": align,
            "verticalAlign": "top",
            "containerId": None,
            "originalText": text,
            "lineHeight": 1.25,
        }
    )
    return element


def arrow(element_id: str, x: float, y: float, dx: float, dy: float) -> dict:
    """Return a straight arrow from (x, y) by (dx, dy)."""
    element = base(element_id, "arrow", x, y, abs(dx), abs(dy), ARROW, "transparent")
    element.update(
        {
            "points": [[0, 0], [dx, dy]],
            "lastCommittedPoint": None,
            "startBinding": None,
            "endBinding": None,
            "startArrowhead": None,
            "endArrowhead": "arrow",
        }
    )
    return element


def build() -> dict:
    """Assemble the scene."""
    elements: list[dict] = []

    # Three columns: exports, app core, desktop shell.
    elements += box("sources", 60, 190, 340, 100, INPUT, "X archive, CSV, JSON")
    elements += box("ingestion", 60, 450, 340, 100, INPUT, "ingestion parsers")
    elements += box("services", 580, 190, 360, 120, RUNTIME, "eterea-app services")
    elements += box("sqlite", 580, 500, 360, 110, STORE, "SQLite bookmarks.db")
    elements += box("shell", 1100, 190, 360, 110, RUNTIME, "Dioxus desktop shell")
    elements += box("views", 1100, 450, 360, 100, RUNTIME, "library, search, settings")

    # Import path, then the service boundary between the shell and storage.
    elements.append(arrow("parse-arrow", 230, 290, 0, 160))
    elements.append(arrow("rows-arrow", 400, 500, 180, -250))
    elements.append(arrow("write-arrow", 720, 310, 0, 190))
    elements.append(arrow("query-arrow", 800, 500, 0, -190))
    elements.append(arrow("data-arrow", 940, 230, 160, 0))
    elements.append(arrow("action-arrow", 1100, 270, -160, 0))
    elements.append(arrow("render-arrow", 1280, 300, 0, 150))

    # Arrow captions sit beside the line so the stroke stays readable.
    elements.append(label("parse-label", 250, 356, 100, "parse", align="left"))
    elements.append(label("rows-label", 500, 389, 110, "parsed rows"))
    elements.append(label("write-label", 590, 390, 110, "writes", align="right"))
    elements.append(label("query-label", 820, 390, 110, "queries", align="left"))
    elements.append(label("data-label", 960, 190, 120, "page data"))
    elements.append(label("action-label", 970, 280, 100, "actions"))
    elements.append(label("render-label", 1300, 360, 100, "renders", align="left"))

    # Front matter and source note.
    elements.append(label("title", 60, 40, 1200, "Desktop reads and writes go through eterea-app", INK, TITLE_FONT, "left"))
    elements.append(label("subtitle", 60, 92, 800, "Eterea v0.1.0 desktop path", MUTED, NOTE_FONT, "left"))
    elements.append(label("sqlite-note", 580, 628, 360, "default path: platform app-data dir", MUTED))
    elements.append(label("source-note", 60, 760, 900, "Source: src/dioxus-app, src/app, src/backend", MUTED, NOTE_FONT, "left"))

    return {
        "type": "excalidraw",
        "version": 2,
        "source": "https://excalidraw.com",
        "elements": elements,
        "appState": {
            "viewBackgroundColor": "#ffffff",
            "exportWithDarkMode": False,
        },
        "files": {},
    }


def main() -> None:
    """Write the scene next to this script."""
    SCENE_PATH.write_text(json.dumps(build(), indent=2) + "\n", encoding="utf-8")
    print(f"wrote {SCENE_PATH}")


if __name__ == "__main__":
    main()
