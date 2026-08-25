#!/usr/bin/env python3
"""Serve ui/ as a look-only preview of WebDesk, with no server behind it.

The real app needs Linux, PAM and a login shell. None of that is interesting
when the change under review is a colour, a gap or a label -- so this serves
the same ui/ files a release would embed, and hands the browser a shim that
answers every /api call and the terminal socket with canned data.

Nothing under ui/ is modified or copied: index.html is rewritten in flight to
pull in the shim, so what renders is the file that ships. Saving any file in
ui/ reloads the open tab.

    scripts/preview.py            # http://127.0.0.1:6868
    scripts/preview.py --port 7000 --no-open

Standard library only -- no npm, no cargo, no venv.
"""

import argparse
import http.server
import json
import re
import socketserver
import sys
import threading
import webbrowser
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
UI = ROOT / "ui"
DEV = Path(__file__).resolve().parent / "preview"

# Everything the harness adds lives under this prefix, so it can never collide
# with a real asset path and nothing has to be filtered back out.
PREFIX = "/__preview"

INJECT_HEAD = f'<link rel="stylesheet" href="{PREFIX}/devtools.css">'
# The shim has to be in place before app.js runs: app.js boots on load and
# immediately calls /api/me.
INJECT_BEFORE_APP = f'<script src="{PREFIX}/mock.js"></script>'
INJECT_BODY_END = f'<script src="{PREFIX}/devtools.js"></script>'


# --------------------------------------------------------------- selectors

SELECTOR_LINE = re.compile(r"^([.#\[a-zA-Z][^{}@]*?)\s*\{", re.MULTILINE)


def css_index():
    """Map every class used in ui/style.css to the lines that style it.

    This is what makes the inspector worth having: clicking a pixel should say
    which lines to name in a prompt, not just which element was hit.
    """
    index = {}
    try:
        text = (UI / "style.css").read_text(encoding="utf-8")
    except OSError:
        return index

    for match in SELECTOR_LINE.finditer(text):
        selector = " ".join(match.group(1).split())
        line = text.count("\n", 0, match.start()) + 1
        for cls in set(re.findall(r"\.([A-Za-z0-9_-]+)", selector)):
            index.setdefault(cls, []).append({"selector": selector, "line": line})
    return index


def js_index():
    """Map class names to the ui/app.js lines that build or query them."""
    index = {}
    try:
        lines = (UI / "app.js").read_text(encoding="utf-8").splitlines()
    except OSError:
        return index

    for n, line in enumerate(lines, 1):
        # Class names only ever appear inside a string in app.js -- as a
        # className assignment, a class="" in a template, or a querySelector.
        for chunk in re.findall(r"""['"`]([^'"`\n]{1,240})['"`]""", line):
            for token in re.findall(r"[A-Za-z][A-Za-z0-9_-]*", chunk):
                if token in CSS_CLASSES:
                    index.setdefault(token, [])
                    if len(index[token]) < 6 and not any(
                        e["line"] == n for e in index[token]
                    ):
                        index[token].append({"line": n, "text": line.strip()[:160]})
    return index


CSS_CLASSES = set()


def build_index():
    global CSS_CLASSES
    css = css_index()
    CSS_CLASSES = set(css)
    return {"css": css, "js": js_index()}


# ------------------------------------------------------------------ reload

WATCH = [UI, DEV]


def fingerprint():
    """Cheap change token: every watched file's size and mtime."""
    parts = []
    for base in WATCH:
        for path in sorted(base.rglob("*")):
            if path.is_file() and "vendor" not in path.parts:
                st = path.stat()
                parts.append(f"{path}:{st.st_mtime_ns}:{st.st_size}")
    return str(hash("|".join(parts)))


# ------------------------------------------------------------------ server


def app_stub(slug):
    """Stand in for a proxied container app, which needs a real host to exist.

    Deliberately plain: it is here to prove the frame is wired up and sized,
    not to imitate any particular application.
    """
    name = slug.replace("-", " ").title()
    return f"""<!doctype html><meta charset=utf-8><title>{name}</title>
<style>
 html{{color-scheme:dark}}
 body{{margin:0;height:100vh;display:grid;place-content:center;gap:.5rem;
   text-align:center;background:#11161d;color:#e6eaf0;
   font:15px/1.6 system-ui,sans-serif}}
 h1{{margin:0;font-size:1.1rem;font-weight:600}}
 p{{margin:0;color:#93a1b1;font-size:.85rem}}
 code{{background:#0d1117;border:1px solid #242c38;border-radius:4px;padding:1px 5px;
   font-size:.8rem}}
</style>
<h1>{name}</h1>
<p>Preview stand-in for the app served at <code>/app/{slug}/</code>.</p>
<p>On a real host this frame is the container's own web interface.</p>
"""


class Handler(http.server.SimpleHTTPRequestHandler):
    # Quiet: a hot-reload poll every 500ms would otherwise bury real requests.
    def log_message(self, fmt, *args):
        if getattr(self, "_quiet", False):
            return
        sys.stderr.write("  %s\n" % (fmt % args))

    def send_payload(self, body, ctype, status=200):
        if isinstance(body, str):
            body = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        # A preview that serves a stale file is worse than a slow one.
        self.send_header("Cache-Control", "no-store, must-revalidate")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = self.path.split("?", 1)[0]

        if path == f"{PREFIX}/reload":
            self._quiet = True
            return self.send_payload(fingerprint(), "text/plain")

        if path == f"{PREFIX}/index":
            return self.send_payload(json.dumps(build_index()), "application/json")

        if path.startswith(PREFIX + "/"):
            return self.serve_file(DEV / path[len(PREFIX) + 1:])

        if path in ("/", "/index.html"):
            return self.serve_index()

        # A container app's window is an iframe pointed at the proxy, which
        # only exists on a real host. Serve a stand-in so the window, its title
        # bar and its two extra buttons can be looked at.
        if path.startswith("/app/"):
            slug = path[len("/app/"):].strip("/").split("/")[0]
            return self.send_payload(app_stub(slug), "text/html; charset=utf-8")

        # Anything the browser asks for that is not a real ui/ file is an API
        # call the shim should have caught. Say so loudly rather than 404ing --
        # a silent miss looks like a UI bug.
        target = UI / path.lstrip("/")
        if not target.is_file():
            return self.send_payload(
                json.dumps({"error": f"no mock for {path}"}),
                "application/json",
                status=501,
            )
        return self.serve_file(target)

    # The shim answers these in the browser; reaching the server means it
    # missed one. Report it the same way.
    def do_POST(self):
        self.send_payload(
            json.dumps({"error": f"no mock for POST {self.path}"}),
            "application/json",
            status=501,
        )

    do_PUT = do_POST

    def serve_index(self):
        html = (UI / "index.html").read_text(encoding="utf-8")
        html = html.replace("</head>", INJECT_HEAD + "\n</head>", 1)
        html = html.replace(
            '<script src="/app.js"></script>',
            INJECT_BEFORE_APP + '\n<script src="/app.js"></script>\n' + INJECT_BODY_END,
            1,
        )
        self.send_payload(html, "text/html; charset=utf-8")

    def serve_file(self, path):
        path = path.resolve()
        if not path.is_file() or not (
            str(path).startswith(str(UI)) or str(path).startswith(str(DEV))
        ):
            return self.send_payload("not found", "text/plain", status=404)
        ctype = {
            ".html": "text/html; charset=utf-8",
            ".js": "text/javascript; charset=utf-8",
            ".css": "text/css; charset=utf-8",
            ".svg": "image/svg+xml",
            ".json": "application/json",
            # The release serves this via mime_guess; without it here a
            # preloaded font is octet-stream and the browser drops the preload.
            ".woff2": "font/woff2",
        }.get(path.suffix, "application/octet-stream")
        self.send_payload(path.read_bytes(), ctype)


class Server(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", type=int, default=6868)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--no-open", action="store_true", help="do not open a browser")
    args = ap.parse_args()

    if not (UI / "index.html").is_file():
        sys.exit(f"no ui/index.html under {ROOT} -- run this from the WebDesk repo")

    build_index()
    url = f"http://{args.host}:{args.port}/"

    with Server((args.host, args.port), Handler) as httpd:
        print(f"WebDesk UI preview -- {url}")
        print(f"  serving {UI} (unmodified), mocks from {DEV}")
        print("  edit ui/style.css or ui/app.js and the tab reloads itself")
        print("  ctrl-c to stop\n")
        if not args.no_open:
            threading.Timer(0.4, lambda: webbrowser.open(url)).start()
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nstopped")


if __name__ == "__main__":
    main()
