#!/usr/bin/env python3
"""Refresh the application marks in ui/ui-icons.svg from Simple Icons.

Run by hand, like vendor-icons.py, and the result is committed -- neither a
build nor an install ever fetches anything.

    scripts/brand-icons.py            # rewrite the marks in place
    scripts/brand-icons.py --check    # fail if the committed sprite is stale

Simple Icons ships one solid path per brand on a 24x24 grid with no fill
attribute. WebDesk's own icons are hairline strokes in currentColor, and a
brand mark is never going to be that drawing. What it can do is share the
grid, take its colour from currentColor like everything else, and be scaled a
little under full size so a solid glyph does not outweigh a stroked one beside
it in the dock. Brand colours are deliberately dropped: an icon takes the
colour of the control it sits in, which is the whole point of currentColor.

The icons are CC0 1.0. The marks themselves remain the trademarks of their
owners; this uses them to label the application they belong to and nothing
else.
"""

from __future__ import annotations

import argparse
import re
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPRITE = ROOT / "ui" / "ui-icons.svg"
SOURCE = "https://raw.githubusercontent.com/simple-icons/simple-icons/develop/icons/{slug}.svg"

# symbol id, Simple Icons slug, scale
#
# The scales are eyeballed against the stroked icons rather than computed: a
# glyph that fills its box (ONLYOFFICE) needs pulling in further than one that
# is mostly outline (Inkscape).
ICONS = [
    ("a-firefox", "firefoxbrowser", 0.80),
    ("a-onlyoffice", "onlyoffice", 0.84),
    ("a-inkscape", "inkscape", 0.88),
    ("a-intellij", "intellijidea", 1.00),
    ("a-vscodium", "vscodium", 0.88),
]

# Not here, and it is worth writing down why so nobody adds it back:
#
#   helium -- Simple Icons' `helium` is the Helium Network, helium.com, a
#   LoRaWAN and crypto company. The application in the catalog is the Chromium
#   fork at helium.computer, which has nothing to do with it beyond the word.
#   There is no Simple Icons entry for that one, so `a-helium` stays hand-drawn
#   in the sprite. Matching on a name alone is how you ship somebody else's
#   logo.
NOT_FROM_SIMPLE_ICONS = ["a-helium", "a-terminal"]

# IntelliJ's mark is a filled square with the letters knocked out of it, which
# at dock size reads as a black block rather than an icon. The square goes and
# the letters stay. Dropping the leading subpath is safe: `z` returns the pen
# to 0,0, so the relative `m` that follows means the same with or without it.
STRIP_LEADING = {"intellijidea": "M0 0v24h24V0z"}

MARKER = "<!-- Application marks, from simpleicons.org"


def fetch(slug: str) -> str:
    with urllib.request.urlopen(SOURCE.format(slug=slug), timeout=30) as r:
        return r.read().decode("utf-8")


def path_of(slug: str, svg: str) -> str:
    ds = re.findall(r'<path[^>]*\sd="([^"]+)"', svg)
    if len(ds) != 1:
        sys.exit(f"{slug}: expected exactly one path, found {len(ds)}")
    d = ds[0]
    lead = STRIP_LEADING.get(slug)
    if lead:
        if not d.startswith(lead):
            sys.exit(f"{slug}: expected leading {lead!r} but got {d[:40]!r} -- "
                     "the upstream icon changed, check it by eye before trusting this")
        d = "M" + d[len(lead) + 1:]
    return d


def symbols() -> list[str]:
    out = []
    for sid, slug, scale in ICONS:
        d = path_of(slug, fetch(slug))
        t = f"translate(12 12) scale({scale}) translate(-12 -12)"
        out.append(
            f'<symbol id="{sid}" viewBox="0 0 24 24">'
            f'<g fill="currentColor" transform="{t}">'
            f'<path d="{d}" /></g></symbol>'
        )
    return out


def splice(sprite: str, marks: list[str]) -> str:
    lines = sprite.splitlines()
    # Everything from the marker to the closing tag is ours to replace.
    try:
        start = next(i for i, ln in enumerate(lines) if ln.startswith(MARKER))
    except StopIteration:
        sys.exit(f"{SPRITE}: no marker line starting {MARKER!r}")
    end = lines.index("</svg>")
    header = [ln for ln in lines[start:end] if not ln.startswith("<symbol ")]
    return "\n".join(lines[:start] + header + marks + lines[end:]) + "\n"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true",
                    help="exit non-zero if the committed sprite is out of date")
    args = ap.parse_args()

    current = SPRITE.read_text()
    updated = splice(current, symbols())

    if args.check:
        if current != updated:
            print(f"{SPRITE} is stale; run scripts/brand-icons.py", file=sys.stderr)
            sys.exit(1)
        print("sprite is up to date")
        return

    SPRITE.write_text(updated)
    print(f"refreshed {len(ICONS)} marks in {SPRITE.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
