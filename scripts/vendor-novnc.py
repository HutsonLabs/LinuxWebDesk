#!/usr/bin/env python3
"""Vendor the noVNC client's core into ui/vendor/novnc/.

Run by hand when the RFB client needs refreshing:

    python3 scripts/vendor-novnc.py

The output is committed, so a build and an install never reach the network for
this -- the same bargain ui/vendor/xterm.js already makes. WebDesk has no npm
and no build step; whatever the browser loads is a file in this repository that
someone read before it was committed.

WHY A PIN AND NOT A RANGE. There is nothing here to resolve a range with: the
files are copied in and committed, so "the newest 1.x" would mean the tree
silently becoming different bytes on whichever afternoon this script was next
run, with no commit saying so. A version is written down instead, and moving it
is an edit somebody reviews. It is also a real API pin: ui/app.js drives this
version's RFB directly -- `scaleViewport`, `resizeSession`, the `clipboard` and
`disconnect` events, and the option `wsProtocols: []` that keeps it off the
websockify subprotocol -- and those are exactly the things noVNC has changed
between minor releases before.

WHAT IS TAKEN. The transitive closure of relative imports from `core/rfb.js`,
and nothing else. Computed rather than listed, so the set cannot drift out of
step with what the browser will actually ask for: add a decoder upstream and it
comes along, drop one and it stops being copied.

WHAT IS LEFT BEHIND, AND WHY.

  app/       noVNC's own client UI -- its control bar, settings panel, styles
             and images. WebDesk draws the window; a second toolbar inside it
             is the thing that gives away a desktop inside a desktop, which is
             the whole point of the auto-hiding title bar in ui/app.js.
  po/        translations for that UI, which is not here to be translated.
  tests/     its Karma suite, plus the browser-runner configuration for it.
  utils/     `novnc_proxy` and friends: a websockify launcher. src/rfb.rs is
             the proxy, and it speaks to a unix socket rather than a port.
  *.html     its own pages. WebDesk serves one document.
  docs, .github, snap, and the rest of the repository furniture.

The upstream layout is preserved verbatim, including `vendor/pako`, because the
imports between these files are relative. Flattening them would mean editing
upstream source, and then every future refresh would mean editing it again.
"""
import io
import os
import re
import shutil
import sys
import tarfile
import urllib.request

VERSION = "1.6.0"
URL = f"https://github.com/novnc/noVNC/archive/refs/tags/v{VERSION}.tar.gz"

# The one module ui/app.js imports. Everything else is here because this one
# reaches it.
ENTRY = "core/rfb.js"

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, os.pardir, "ui", "vendor", "novnc")

# Anchored to the start of a line, because a static import can only appear
# there -- and because a looser pattern reads `* from 'ar' written by ...` in a
# comment in pako's trees.js as an import of a module called `ar`. The import
# form may run over several lines, so its own scan does not stop at one; the
# re-export form is a single line and is held to it.
IMPORT = re.compile(r"""^[ \t]*import\b[^;'"]*['"]([^'"]+)['"]""", re.M)
REEXPORT = re.compile(r"""^[ \t]*export\b[^;'"\n]*\bfrom\s*['"]([^'"]+)['"]""", re.M)


def fetch():
    req = urllib.request.Request(URL, headers={"User-Agent": "webdesk-vendor"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return tarfile.open(fileobj=io.BytesIO(r.read()), mode="r:gz")


def closure(read, entry):
    """Every module reachable from `entry` by a relative import, entry first."""
    found, queue = [], [entry]
    seen = set()
    while queue:
        rel = queue.pop(0)
        if rel in seen:
            continue
        seen.add(rel)
        src = read(rel)
        if src is None:
            raise ValueError(f"{rel} is imported but not in the tarball")
        found.append((rel, src))
        for target in IMPORT.findall(src) + REEXPORT.findall(src):
            # A bare specifier would need a resolver -- an import map, or the
            # bundler this project does not have. noVNC has none today, and if
            # it grows one that is a decision to take deliberately rather than
            # to discover as a 404 in somebody's browser.
            if not target.startswith("."):
                raise ValueError(f"{rel} imports {target!r}, which is not relative")
            queue.append(os.path.normpath(os.path.join(os.path.dirname(rel), target)))
    return found


def main():
    print(f"noVNC {VERSION}")
    try:
        tar = fetch()
    except Exception as e:
        print(f"  FAILED to fetch {URL}: {e}", file=sys.stderr)
        return 1

    root = f"noVNC-{VERSION}"
    names = set(tar.getnames())

    def read(rel):
        name = f"{root}/{rel}"
        if name not in names:
            return None
        return tar.extractfile(name).read().decode()

    try:
        modules = closure(read, ENTRY)
    except Exception as e:
        print(f"  FAILED: {e}", file=sys.stderr)
        return 1

    # Written from scratch each time. Keeping what was here before would leave
    # a module upstream has deleted sitting in the tree, still served, still
    # committed, and imported by nothing.
    shutil.rmtree(OUT, ignore_errors=True)

    total = 0
    for rel, src in sorted(modules):
        path = os.path.join(OUT, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(src)
        total += len(src.encode())
        print(f"  {len(src.encode()):>7}  {rel}")

    # The licence rides alongside the code, the way ui/icons.LICENSE does for
    # the sprite. Two of them: noVNC's core is MPL 2.0, and the copy of pako it
    # carries -- which the zlib decoders reach into -- is MIT.
    licence = (
        f"noVNC {VERSION}, vendored into WebDesk by scripts/vendor-novnc.py.\n"
        f"{URL}\n"
        "\n"
        "Only the core client is here: the modules core/rfb.js reaches by\n"
        "import, and the copy of pako they use. None of noVNC's own user\n"
        "interface, pages, styles or translations was taken, so the licences\n"
        "below that cover HTML, CSS, fonts and images cover nothing in this\n"
        "directory.\n"
        "\n"
        "=============================== noVNC ===============================\n"
        "\n" + read("LICENSE.txt") + "\n"
        "=========================== vendor/pako =============================\n"
        "\n" + read("vendor/pako/LICENSE")
    )
    with open(os.path.join(OUT, "LICENSE"), "w") as f:
        f.write(licence)
    total += len(licence.encode())

    # The closure proved these resolve inside the tarball. This proves they
    # resolve on disk, at the paths actually written, which is the thing the
    # browser will be asked to do and the one nobody can check by reading.
    broken = 0
    for rel, src in modules:
        for target in IMPORT.findall(src) + REEXPORT.findall(src):
            at = os.path.join(OUT, os.path.dirname(rel), target)
            if not os.path.exists(at):
                print(f"  DANGLING {rel} -> {target}", file=sys.stderr)
                broken += 1

    kept = {rel.split("/", 1)[0] for rel, _ in modules}
    top = sorted({n[len(root) + 1:].split("/")[0] for n in names if "/" in n} - {""})
    print(f"\nwrote ui/vendor/novnc -- {len(modules)} modules, {total} bytes with the licence")
    print(f"entry point: /vendor/novnc/{ENTRY}")
    print("taken from upstream: " + ", ".join(sorted(kept)))
    print("left behind:         " + ", ".join(t for t in top if t not in kept))
    print(f"every import resolves on disk: {'no' if broken else 'yes'}")
    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main())
