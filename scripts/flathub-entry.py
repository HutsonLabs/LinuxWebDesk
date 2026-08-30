#!/usr/bin/env python3
"""Write a `flathub!` catalog entry from a Flathub application id.

    scripts/flathub-entry.py org.gimp.GIMP org.kde.krita
    scripts/flathub-entry.py --search "photo editor"

A streamed entry is short enough that typing one is not the hard part. A
container entry has to answer for a published port, a state directory,
`PUID`/`PGID`, a shared memory size, a clock, a render node and whether the
application survives a path prefix -- seven questions with lookup-able answers.
A streamed entry answers none of them, because the application is already
running as the right user on the right machine. What is left is a judgement
call, so this prints what the judgement needs and then the entry.

**Size is the part that has already decided a catalog question once.**
`intellij-idea` was dropped on size alone: nothing about it was broken, it
simply unpacks to roughly 9 GB, which was more than the free space on the
filesystem the engine stores images in -- so the entry most likely to fail its
own install was also the one whose failure would take the rest of the machine
down with it. Flatpaks are smaller than desktop images, and a runtime is shared
by every application built on it, so the second GNOME app on a host costs its
own size and nothing more. Those are two different kinds of cost, so this prints
them as two numbers rather than one.

**What it cannot answer, and says so every time.** Whether the application is
usable over RFB at a fixed initial resolution, whether it opens dialogs a kiosk
compositor holding one surface handles badly, and whether the session bus names
it asks to talk to matter when it does not get them. Those are answered by
running it. What comes out of here is a draft entry, not a finished one.

**It refuses rather than guessing.** An id Flathub does not have, an id that is
not a desktop application, and an application marked end-of-life all stop here
with a reason.

The icon is always `a-box`, because a brand mark is a separate step: add the
Simple Icons slug to `scripts/brand-icons.py`, re-run it, and change the one
line.

Standard library only, and it reads Flathub over the network every time -- there
is nothing to vendor, because the answer changes when the app is rebuilt.
Nothing here writes a file. The output is meant to be read and then pasted into
`src/catalog.rs` by hand, since a script that edited the catalog would be a
script that could add an entry nobody read.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import textwrap
import urllib.error
import urllib.request

API = "https://flathub.org/api/v2"
UA = "webdesk-flathub-entry"

MIB = 1024 * 1024
GIB = 1024 * MIB

# Two thresholds, and neither is a hard limit -- an entry is a judgement and
# this is only the point at which the judgement has to be made out loud.
#
# 1 GiB is where `df` stops being a formality on a host that also runs the
# desktop images. 4 GiB is where it is the same decision `intellij-idea` lost:
# an application large enough that a failed install is not just a failed install
# but a full filesystem, on the disk everything else on the machine writes to.
NOTE_SIZE = 1 * GIB
WARN_SIZE = 4 * GIB

# The last component of an id that names a category rather than the program.
# `com.obsproject.Studio` is not the `studio` entry, and `io.github.x.Desktop`
# is not the `desktop` one; for these the application's own name is the better
# source and the id's tail is noise.
GENERIC = {
    "app", "application", "browser", "client", "desktop", "editor", "gtk",
    "gui", "linux", "manager", "player", "qt", "studio", "viewer",
}

# A tray icon is a surface a kiosk compositor has nowhere to put. These are the
# bus names an application asks for when it wants one.
TRAY_NAMES = ("StatusNotifierWatcher", "canonical.indicator", "ayatana")

# The indentation an entry is pasted at, inside `pub static CATALOG`.
INDENT = " " * 4

# What a Flatpak application id looks like: reverse DNS, at least one dot. This
# is checked before anything is fetched so that `flathub-entry.py gimp` is told
# to use --search rather than told Flathub has never heard of GIMP.
APP_ID = re.compile(r"^[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+$")


def get(path: str, body: dict | None = None):
    """A GET, or a POST when there is a body. `None` for a clean 404."""
    data = json.dumps(body).encode() if body is not None else None
    headers = {"User-Agent": UA}
    if data:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(f"{API}{path}", data=data, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def wrapped(text: str, first: str, rest: str) -> None:
    # Hyphens are never a break point here: half the sentences below contain a
    # `--`, an `xdg-run` or a `--search`, and none of them survive being split.
    print(textwrap.fill(text, width=79, initial_indent=first, subsequent_indent=rest,
                        break_on_hyphens=False, break_long_words=False))


def field(label: str, text: str) -> None:
    """`  label   text`, wrapped under the text rather than under the label."""
    wrapped(text, f"  {label:<11}", " " * 13)


def bullet(text: str) -> None:
    wrapped(text, "    - ", "      ")


def human(n: int | None) -> str:
    if not n:
        return "unknown"
    if n >= GIB:
        return f"{n / GIB:.1f} GiB"
    return f"{n / MIB:.0f} MiB"


def split_camel(s: str) -> str:
    """LibreOffice -> Libre Office. GIMP and IntelliJ are left alone.

    The break is only taken before an upper-case letter that *starts a word* --
    one followed by a lower-case letter. Splitting on every upper-case letter
    turns `IntelliJ` into `intelli-j`, which is not a slug anybody would type.
    """
    return re.sub(r"(?<=[a-z0-9])(?=[A-Z][a-z])", " ", s)


def slugify(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", split_camel(text).lower()).strip("-")


def slugs(app_id: str, name: str) -> tuple[str, str]:
    """The slug to use, and the other candidate, so a human can pick.

    A slug is a key -- lowercase, hyphenated, safe in a URL and in a unit
    instance name -- and it is the one field here nothing on Flathub knows. The
    id's last component is right most of the time and obviously wrong the rest
    of it, so both candidates are printed and the wrong one is cheap to reject.
    """
    parts = [p for p in app_id.split(".") if p]
    tail = parts[-1] if parts else ""
    from_id = slugify(tail)
    # `LibreOffice` and `libreoffice` are the same word, hyphenated by the camel
    # split and not by anybody's intention. The id's own vendor component says
    # which it is.
    if from_id.replace("-", "") in {p.lower() for p in parts[:-1]}:
        from_id = from_id.replace("-", "")
    from_name = "-".join(slugify(name).split("-")[:3])
    if not from_id or from_id in GENERIC:
        return from_name or from_id, from_id
    return from_id, from_name


def window(app: dict) -> tuple[int, int, str]:
    """1280 wide, and the shape the application's own screenshots are.

    A starting point, not a limit -- it is what `cage` comes up at before the
    browser has said how big its window is. The screenshot is the only thing in
    the metadata that says anything about shape at all, and an application
    photographed at 16:9 is usually an application that wants to be 16:9.
    """
    shots = app.get("screenshots") or []
    shot = next((s for s in shots if s.get("default")), shots[0] if shots else None)
    best = None
    for size in (shot or {}).get("sizes") or []:
        try:
            w, h = int(size["width"]), int(size["height"])
        except (KeyError, TypeError, ValueError):
            continue
        if w > 0 and h > 0 and (best is None or w > best[0]):
            best = (w, h)
    if not best:
        return 1280, 800, "no screenshot to take a shape from, so the middle of the road"
    h = round(1280 * best[1] / best[0] / 16) * 16
    h = min(1000, max(720, h))
    return 1280, h, f"the shape of its own {best[0]}x{best[1]} screenshot"


def rust_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def tagline_of(summary: str) -> str:
    """Flathub writes fragments; the catalog's taglines are sentences."""
    s = " ".join((summary or "").split())
    if s and s[-1] not in ".!?":
        s += "."
    return s


def verification(meta: dict) -> str:
    """Whether Flathub has tied the publisher to the project.

    Not a security control and not treated as one -- an unverified app is not
    malicious, it is unattributed. It matters here because a streamed entry runs
    on the host as a signed-in user rather than inside a container, so "who
    publishes this" is a question with a shorter answer than it used to have.
    """
    if not meta.get("flathub::verification::verified"):
        return ("no. Not a verdict on the software, but nobody has tied this "
                "publisher to the project it claims to be, and a streamed app runs "
                "on the host rather than in a container.")
    method = meta.get("flathub::verification::method")
    site = meta.get("flathub::verification::website")
    login = meta.get("flathub::verification::login_name")
    provider = meta.get("flathub::verification::login_provider")
    if method == "website" and site:
        return f"yes -- the publisher controls {site}"
    if method == "login_provider" and login:
        return f"yes -- published by {login} on {provider or 'a linked account'}"
    return f"yes, by {method or 'a method Flathub did not name'}"


def sandbox_notes(meta: dict) -> list[str]:
    """What the manifest asks for, read as consequences for a streamed entry.

    Declared permissions are what the application asked for, not what it needs;
    an app can ask for PulseAudio and be perfectly usable silent. So these are
    written as things to check, which is what they are.
    """
    perms = meta.get("permissions") or {}
    sockets = set(perms.get("sockets") or [])
    files = set(perms.get("filesystems") or [])
    bus = perms.get("session-bus") or {}
    names = list(bus.get("talk") or []) + list(bus.get("own") or [])
    out = []

    if "pulseaudio" in sockets:
        out.append("asks for PulseAudio. RFB carries no audio, so this app will "
                   "be silent here -- fine for an editor, not for a media player.")
    if "wayland" not in sockets:
        kind = "x11" if "x11" in sockets or "fallback-x11" in sockets else "neither"
        out.append(f"declares {kind}, not wayland. Under `cage` that means Xwayland, "
                   "which is a second thing that has to be present and working.")
    if any(n for n in names if any(t in n for t in TRAY_NAMES)):
        out.append("asks for a status-notifier bus name, i.e. it wants a system "
                   "tray. There is no tray in a one-surface kiosk compositor, and "
                   "an app that hides in one instead of exiting is a bad fit.")
    if "host" in files or "home" in files:
        out.append("asks for the whole of `home` or `host`. Its sandbox is then no "
                   "narrower than the containers' on the filesystem question -- it "
                   "is still the *right user's* files, which is the point, but do "
                   "not claim finer-grained confinement for this one.")
    if "all" in set(perms.get("devices") or []):
        out.append("asks for `devices=all`, which is every device node including "
                   "input and USB, not just the render node.")
    if names and not any(any(t in n for t in TRAY_NAMES) for n in names):
        one = len(names) == 1
        out.append(f"asks to talk to {len(names)} session bus "
                   f"{'name, which is' if one else 'names, starting with'} "
                   f"{names[0]}. A streamed session has a bus of its own but not the "
                   "desktop services behind those names, so check what the app does "
                   "when nothing answers.")
    return out


def entry(slug: str, name: str, app_id: str, tagline: str, w: int, h: int) -> str:
    lines = [
        "flathub!(",
        f"    {rust_str(slug)},",
        f"    {rust_str(name)},",
        f"    {rust_str(app_id)},",
        '    "a-box",',
        f"    {rust_str(tagline)},",
        f"    {w} x {h},",
        "),",
    ]
    return "\n".join(INDENT + ln for ln in lines)


def describe(app_id: str) -> bool:
    """One application: what decides it, then the entry. False if refused."""
    def refuse(why: str) -> bool:
        print(app_id)
        field("REFUSED", why)
        print()
        return False

    if not APP_ID.match(app_id):
        return refuse("that is not the shape of a Flatpak application id, which is "
                      "reverse DNS -- `org.gimp.GIMP`, not `gimp`. Find the id with "
                      "--search, or read it off the app's page on flathub.org.")

    app = get(f"/appstream/{app_id}")
    if app is None:
        return refuse("Flathub has no application with this id. Ids are "
                      "case-sensitive and often end in a capitalised word -- check "
                      f"https://flathub.org/apps/{app_id}, or find it with --search.")

    kind = app.get("type") or "unstated"
    if kind != "desktop-application":
        return refuse(f"this is a {kind}, not a desktop application. A streamed "
                      "entry is a window on the dock, and a runtime or an add-on "
                      "has none to draw.")
    if app.get("is_eol"):
        return refuse("marked end-of-life on Flathub. `flatpak update` being a real "
                      "upgrade path is most of why a streamed entry is worth having, "
                      "and this one has no more updates coming.")

    summary = get(f"/summary/{app_id}") or {}
    meta = summary.get("metadata") or {}
    name = app.get("name") or app_id
    tagline = tagline_of(app.get("summary") or "")
    slug, other = slugs(app_id, name)
    w, h, why = window(app)

    size = summary.get("installed_size") or 0
    runtime_size = meta.get("runtimeInstalledSize") or 0
    licence = app.get("project_license") or "unstated"
    free = "free" if app.get("is_free_license") else "NOT a free licence"

    print(app_id)
    wrapped(f"{name} -- {app.get('summary') or ''}", "  ", "  ")
    field("licence", f"{licence} ({free})")
    field("verified", verification(app.get("metadata") or {}))
    field("runtime", (meta.get("runtimeName") or meta.get("runtime") or "unknown")
          + (" -- END OF LIFE, and an app on a dead runtime stops getting fixes "
             "even when the app itself is maintained" if meta.get("runtimeIsEol") else ""))
    # Two numbers, not one, because they are not the same kind of cost. The
    # application is what this entry adds; the runtime is what it adds only if
    # no other app on the host is already built on it.
    runtime_cost = (f"on {human(runtime_size)} of runtime that every other app built "
                    "on it shares" if runtime_size
                    else "on a runtime Flathub did not put a size on")
    field("size", f"{human(size)} of application, {runtime_cost}")
    if size + runtime_size >= WARN_SIZE:
        field("SIZE", f"{human(size + runtime_size)} the first time, which is the "
                      "decision `intellij-idea` lost. Look at `df` on the filesystem "
                      "Flatpak stores apps in before adding this, not after.")
    elif size >= NOTE_SIZE:
        field("", f"{human(size)} is large enough that `df` is worth a look first.")
    field("window", f"{w} x {h} -- {why}")
    field("slug", slug + (f"   (the other candidate was `{other}`)"
                          if other and other != slug else ""))
    if len(name) > 20:
        field("name", f"{len(name)} characters, which is a dock label nobody can read. "
                      "The catalog's names are what an app is called, not what it is "
                      "registered as -- shorten it.")

    print()
    print(entry(slug, name, app_id, tagline, w, h))
    print()

    print("  this tool cannot answer, and somebody has to by running it:")
    bullet("whether it is usable at a fixed initial resolution over RFB. Resize is "
           "the roughest edge of the streamed transport, and an app that lays itself "
           "out once will look wrong the moment the window changes.")
    bullet("whether its dialogs behave. `cage` holds one surface, so a second "
           "top-level window is a question about the compositor rather than about "
           "the app.")
    bullet("whether the tagline above reads like the rest of the catalog. It is "
           "Flathub's own summary with a full stop added, which is a starting point "
           "and not a sentence anybody wrote for this dock.")
    for note in sandbox_notes(meta):
        bullet(note)
    bullet("the icon is `a-box`. If it deserves a mark of its own, add the Simple "
           "Icons slug to scripts/brand-icons.py, re-run it, and change the one line.")
    print()
    return True


def search(query: str, limit: int) -> int:
    """Find ids by what an application does.

    Flathub's search *ranks* the whole catalog rather than filtering it: the
    reported total is 3315 for every query, and a query of `zzzqqq` still comes
    back with twenty-one applications. So the count is not printed -- it would be
    a number that means nothing -- and the output says out loud that a result is
    only the best match, not necessarily a match.
    """
    res = get("/search", {"query": query}) or {}
    hits = [h for h in (res.get("hits") or []) if h.get("type") == "desktop-application"]
    if not hits:
        print(f'nothing on Flathub ranks for "{query}".')
        return 1

    print(f'"{query}" -- the {min(limit, len(hits))} desktop applications Flathub '
          "ranks highest.")
    print("This is a ranking and not a filter: it answers even when nothing "
          "matches.\n")
    for hit in hits[:limit]:
        app_id = hit.get("app_id") or ""
        # One extra request per hit, because the search index carries no size and
        # size is the first thing worth knowing about a candidate.
        s = get(f"/summary/{app_id}") or {}
        mark = "verified" if hit.get("verification_verified") else "unverified"
        print(f"  {app_id}")
        wrapped(f"{hit.get('name')} -- {hit.get('summary')}", "    ", "    ")
        print(f"    {human(s.get('installed_size')):>8}  {mark}  "
              f"{hit.get('runtime') or 'runtime unstated'}")
    wrapped("Sizes here are the application alone. Run this again with an id for the "
            "runtime it sits on, the licence, the sandbox it asks for and an entry to "
            "paste.", "\n", "")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description="Draft a flathub! catalog entry from a Flathub application id.",
        epilog="The entry is printed, never written. Paste it into src/catalog.rs.",
    )
    ap.add_argument("ids", nargs="*", metavar="ID",
                    help="Flathub application ids, e.g. org.gimp.GIMP")
    ap.add_argument("--search", metavar="QUERY",
                    help="find application ids by what the app does")
    ap.add_argument("--limit", type=int, default=8, metavar="N",
                    help="how many search results to show (default 8)")
    args = ap.parse_args()

    if not args.search and not args.ids:
        ap.error("give one or more application ids, or --search QUERY")

    if args.search:
        rc = search(args.search, args.limit)
        if not args.ids:
            return rc

    ok = sum(describe(i) for i in args.ids)
    refused = len(args.ids) - ok
    if args.ids:
        print(f"{ok} entr{'y' if ok == 1 else 'ies'} drafted"
              + (f", {refused} refused" if refused else "")
              + ". Nothing was written -- paste what you want into src/catalog.rs.")
    return 1 if refused else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except urllib.error.URLError as e:
        # Flathub is the only source here, so there is no degraded mode to fall
        # back to: without it there is no size, no licence and no name to put in
        # an entry, and inventing any of them is worse than stopping.
        sys.exit(f"cannot reach Flathub: {e.reason}")
