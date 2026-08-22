#!/bin/sh
# LinuxWebDesk bootstrap installer.
#
#   curl -fsSL https://raw.githubusercontent.com/HutsonLabs/LinuxWebDesk/main/bootstrap.sh | sudo sh
#
# Fetches the source for a ref, puts it in a fixed place, and runs install.sh.
# This is also the engine behind the in-browser update: the updater re-runs a
# copy of this script, so there is exactly one implementation of "fetch and
# install" and the update path is the install path.
#
# Deliberately POSIX sh with no dependency beyond curl and tar, because it has
# to run before anything of ours exists on the host.
#
# Knobs (all optional):
#   LWD_REPO=owner/name      source repository        (default HutsonLabs/LinuxWebDesk)
#   LWD_REF=branch|tag|sha   what to install          (default main)
#   LWD_SRC_DIR=/path        where the source lives   (default /usr/local/src/linuxwebdesk)
#   PORT=7788                listen port
#   PREFIX=/usr/local/bin    where the binary goes
#   FORCE_BUILD=1            rebuild even if a binary is already present
set -eu

REPO=${LWD_REPO:-HutsonLabs/LinuxWebDesk}
REF=${LWD_REF:-main}
SRC_DIR=${LWD_SRC_DIR:-/usr/local/src/linuxwebdesk}

say() { echo "==> $*"; }
die() { echo "!! $*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || die "LinuxWebDesk only runs on Linux (this is $(uname -s))"

if [ "$(id -u)" -ne 0 ]; then
  echo "!! this installer must run as root." >&2
  echo "   curl -fsSL https://raw.githubusercontent.com/$REPO/$REF/bootstrap.sh | sudo sh" >&2
  exit 1
fi

for tool in curl tar; do
  command -v "$tool" >/dev/null 2>&1 || die "need $tool on PATH"
done

# --- resolve the ref to a commit -------------------------------------------
# Asking for the `sha` media type gets the bare commit id back as plain text,
# which saves parsing JSON in sh. Pinning to the resolved commit also means the
# download and the recorded version cannot disagree if the branch moves while
# we are working.
say "resolving $REPO@$REF"
SHA=$(curl -fsSL --max-time 20 \
        -H 'Accept: application/vnd.github.sha' \
        -H 'User-Agent: linuxwebdesk-bootstrap' \
        "https://api.github.com/repos/$REPO/commits/$REF" 2>/dev/null || true)

case "$SHA" in
  *[!0-9a-f]* | "")
    # Rate-limited, offline, or a private repo. The tarball endpoint takes a
    # ref directly, so the install can still proceed -- just unpinned.
    say "could not resolve a commit id; falling back to the ref itself"
    SHA=""
    DOWNLOAD_REF=$REF
    ;;
  *)
    say "    $SHA"
    DOWNLOAD_REF=$SHA
    ;;
esac

# --- fetch ------------------------------------------------------------------
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

say "downloading $REPO@$DOWNLOAD_REF"
curl -fsSL --max-time 300 \
  -H 'User-Agent: linuxwebdesk-bootstrap' \
  "https://codeload.github.com/$REPO/tar.gz/$DOWNLOAD_REF" -o "$TMP/src.tar.gz" \
  || die "download failed -- check the repository name, ref, and network"

say "extracting"
tar -xzf "$TMP/src.tar.gz" -C "$TMP"
TOP=$(tar -tzf "$TMP/src.tar.gz" | head -1 | cut -d/ -f1)
[ -n "$TOP" ] && [ -d "$TMP/$TOP" ] || die "unexpected archive layout"
[ -f "$TMP/$TOP/install.sh" ] || die "archive has no install.sh; is $REPO right?"

# --- place the source -------------------------------------------------------
# Everything except target/ is replaced. Keeping the build directory is what
# makes a later update a few minutes rather than a cold build every time.
say "installing source into $SRC_DIR"
mkdir -p "$SRC_DIR"
find "$SRC_DIR" -mindepth 1 -maxdepth 1 ! -name target -exec rm -rf {} +
cp -a "$TMP/$TOP/." "$SRC_DIR/"

# After the copy, not before: `cp -a src/. dst/` carries the source directory's
# own mode onto dst, and GitHub's tarballs ship their top directory as 0775.
# This tree is compiled and installed as root and has no business being
# group-writable, whatever the archive or the caller's umask says.
chmod 0755 "$SRC_DIR"

# build.rs reads this. A tarball carries no git metadata, so without it the
# running binary could not tell you which commit it is, and the update check
# would have nothing to compare against.
cat > "$SRC_DIR/.lwd-source" <<EOF
repo=$REPO
ref=$REF
commit=$SHA
EOF

# --- build and install ------------------------------------------------------
say "handing over to install.sh"
# Exported rather than passed as a prefix to `exec`: assignments in front of a
# special built-in are not reliably placed in the new image's environment, and
# install.sh silently falling back to the default ref would be a quiet way to
# install one thing and track another.
export LWD_REPO=$REPO
export LWD_REF=$REF
export LWD_SRC_DIR=$SRC_DIR
exec bash "$SRC_DIR/install.sh"
