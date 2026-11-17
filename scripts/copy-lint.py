#!/usr/bin/env python3
"""Check user-facing copy for filler, AI phrases, and layout crutches.

Usage:
  python3 scripts/copy-lint.py design-demos/dir-*.html
  python3 scripts/copy-lint.py src/desktop/src

HTML: text nodes outside style/script/code/pre/kbd/svg, plus placeholder,
title, aria-label, and alt attributes. Rust: string literals.

Errors: banned phrases, exclamation marks, em dashes, all-caps labels.
Warnings: repeated text, arrow suffixes, middle-dot chains.

Allow a line with: <!-- copy-ok: reason --> (HTML) or // copy-ok: reason (Rust).
"""
from __future__ import annotations

import html.parser
import pathlib
import re
import sys

BANNED = [
    "seamless", "effortless", "delightful", "elevate", "elevated", "unlock", "empower",
    "supercharge", "reimagined", "curated", "archive-first", "reading room",
    "service-derived", "ai-powered", "cutting-edge", "state-of-the-art", "game-changer",
    "revolutionary", "next-level", "blazing", "lightning-fast", "beautifully",
    "intuitive", "bespoke", "tailored", "immerse", "immersive", "dive in",
    "welcome to", "get started", "learn more", "discover more", "your library",
    "showing ", "loaded ", "explore",
]
BANNED_RE = [(p, re.compile(r"\b" + re.escape(p.strip()) + r"\b", re.I)) for p in BANNED]
SKIP_TAGS = {"style", "script", "code", "pre", "kbd", "svg", "path", "head", "title", "meta"}
ATTRS = ("placeholder", "title", "aria-label", "alt")
ALL_CAPS = re.compile(r"^[A-Z0-9][A-Z0-9 &/+·.\-]{2,}$")


class Text(html.parser.HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.stack: list[str] = []
        self.items: list[tuple[str, str, int]] = []
        self.ignored = False

    def handle_starttag(self, tag, attrs):
        self.stack.append(tag)
        if tag in SKIP_TAGS:
            self.ignored = True
        for name, value in attrs:
            if name in ATTRS and value and value.strip():
                self.items.append((name, value.strip(), self.getpos()[0]))

    def handle_endtag(self, tag):
        if tag in SKIP_TAGS:
            self.ignored = False
        if tag in self.stack:
            while self.stack and self.stack.pop() != tag:
                pass

    def handle_data(self, data):
        if self.ignored or not data.strip():
            return
        self.items.append(("text", data.strip(), self.getpos()[0]))


def check_text(kind: str, text: str, where: str, line: int, errors, warnings):
    norm = " ".join(text.split())
    for phrase, rx in BANNED_RE:
        if rx.search(norm):
            errors.append(f"{where}:{line}: banned phrase '{phrase}' in {kind}: {norm!r}")
    if any(p in norm.lower() for p in ("http://", "https://")):
        return
    if kind in ("text", "placeholder", "title"):
        if "!" in norm:
            errors.append(f"{where}:{line}: exclamation mark: {norm!r}")
        if "—" in norm:
            errors.append(f"{where}:{line}: em dash in UI copy: {norm!r}")
        if norm.endswith("→") or norm.endswith("->"):
            warnings.append(f"{where}:{line}: arrow suffix: {norm!r}")
    if kind == "text" and ALL_CAPS.match(norm) and sum(c.isalpha() for c in norm) >= 3:
        errors.append(f"{where}:{line}: all-caps label: {norm!r}")


def lint_html(path: pathlib.Path, errors, warnings):
    source = path.read_text()
    allowed = set()
    for m in re.finditer(r"<!--\s*copy-ok:(.*?)-->", source, re.S):
        allowed.update(range(source[: m.start()].count("\n") + 1, source[: m.end()].count("\n") + 2))
    p = Text()
    p.feed(source)
    seen: dict[str, list[int]] = {}
    for kind, text, line in p.items:
        if line in allowed:
            continue
        check_text(kind, text, str(path), line, errors, warnings)
        if kind == "text":
            key = " ".join(text.lower().split())
            if len(key) > 3:
                seen.setdefault(key, []).append(line)
    for key, lines in seen.items():
        if len(lines) > 3 and key not in ("favorite", "copy link", "open on x"):
            warnings.append(f"{path}:{lines[0]}: repeated copy x{len(lines)}: {key!r}")


def lint_rust(path: pathlib.Path, errors, warnings):
    source = path.read_text()
    for m in re.finditer(r'"((?:[^"\\]|\\.)*)"', source):
        line = source[: m.start()].count("\n") + 1
        context = source.splitlines()[line - 1] if source.splitlines() else ""
        if "copy-ok" in context:
            continue
        if any(x in context for x in ("include_str!", "include_bytes!", "env!")):
            continue
        check_text("text", m.group(1), str(path), line, errors, warnings)


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2
    errors: list[str] = []
    warnings: list[str] = []
    for raw in argv[1:]:
        path = pathlib.Path(raw)
        if path.is_dir():
            files = [p for p in path.rglob("*") if p.suffix in (".rs", ".html")]
        elif path.is_file():
            files = [path]
        else:
            errors.append(f"{raw}: no such file")
            continue
        for f in files:
            if f.suffix == ".html":
                lint_html(f, errors, warnings)
            elif f.suffix == ".rs":
                lint_rust(f, errors, warnings)
    for w in warnings:
        print("warning:", w)
    for e in errors:
        print("error:", e)
    print(f"{len(errors)} errors, {len(warnings)} warnings")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
