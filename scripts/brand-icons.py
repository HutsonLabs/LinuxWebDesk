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
    # `heliumbrowser`, not `helium`. The latter is the Helium Network at
    # helium.com, a LoRaWAN and crypto company that shares only the word with
    # the Chromium fork this catalog installs. Two entries, one obvious name,
    # and picking by name alone ships the wrong company's logo.
    ("a-helium", "heliumbrowser", 0.82),
    ("a-onlyoffice", "onlyoffice", 0.84),
    ("a-gimp", "gimp", 0.86),
    ("a-dbeaver", "dbeaver", 0.86),
    ("a-inkscape", "inkscape", 0.88),
    ("a-vscodium", "vscodium", 0.88),
]

# Marks that live in the sprite but are not Simple Icons' to give. `a-termhut`
# is first-party and comes from its own site repository; Remmina and Disk Usage
# Analyzer are simply not in the set, and their marks were added by hand on the
# 512 grid they were drawn on. All three sit above MARKER in the sprite, which
# is what keeps this script away from them -- the list is documentation, not a
# filter.
NOT_FROM_SIMPLE_ICONS = ["a-termhut", "a-remmina", "a-baobab"]

# Some marks are a filled square with the shape knocked out of it, which at dock
# size reads as a black block rather than an icon. The square goes and the shape
# stays. Dropping the leading subpath is safe when it is a closed rectangle: `z`
# returns the pen to 0,0, so the relative `m` that follows means the same with or
# without it.
#
# Empty since `intellijidea` -- the one mark that needed it -- left with the
# catalog entry it drew. Kept because the next filled-square logo is a line in
# here rather than a change to the script.
STRIP_LEADING: dict[str, str] = {}

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
    """Drop the marks this script owns and write them back after the marker.

    Ownership is by id, not by position. Taking everything between the marker
    and `</svg>` would be simpler and was what this did, but the sprite has
    grown hand-authored symbols below the marks -- a-dockhand, the title bar
    switch, the layout mark -- and that rule quietly deleted them. An id this
    script does not name is left exactly where it was.
    """
    lines = sprite.splitlines()
    owned = tuple(f'<symbol id="{sid}"' for sid, _, _ in ICONS)
    kept = [ln for ln in lines if not ln.startswith(owned)]
    try:
        start = next(i for i, ln in enumerate(kept) if ln.startswith(MARKER))
    except StopIteration:
        sys.exit(f"{SPRITE}: no marker line starting {MARKER!r}")
    # The marker is a multi-line comment; the marks go after it closes.
    try:
        end = next(i for i in range(start, len(kept)) if kept[i].rstrip().endswith("-->"))
    except StopIteration:
        sys.exit(f"{SPRITE}: the comment starting {MARKER!r} is never closed")
    return "\n".join(kept[:end + 1] + marks + kept[end + 1:]) + "\n"


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
