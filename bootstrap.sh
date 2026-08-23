#!/bin/sh
# WebDesk bootstrap installer.
#
#   curl -fsSL https://raw.githubusercontent.com/HutsonLabs/WebDesk/main/bootstrap.sh | sudo sh
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
#   WD_REPO=owner/name      source repository        (default HutsonLabs/WebDesk)
#   WD_REF=branch|tag|sha   what to install          (default main)
#   WD_SRC_DIR=/path        where the source lives   (default /usr/local/src/webdesk)
#   PORT=6767                listen port
#   PREFIX=/usr/local/bin    where the binary goes
#   WD_PREBUILT=off         never use a release binary; always compile here
#   WD_RELEASE_TAG=tag      release to take the binary from (default derived)
#   WD_REQUIRE_ATTESTATION=1  refuse to install unless provenance is verified
#   FORCE_BUILD=1            rebuild even if a binary is already present
set -eu

REPO=${WD_REPO:-HutsonLabs/WebDesk}
REF=${WD_REF:-main}
SRC_DIR=${WD_SRC_DIR:-/usr/local/src/webdesk}

say() { echo "==> $*"; }
die() { echo "!! $*" >&2; exit 1; }

[ "$(uname -s)" = Linux ] || die "WebDesk only runs on Linux (this is $(uname -s))"

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
        -H 'User-Agent: webdesk-bootstrap' \
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
  -H 'User-Agent: webdesk-bootstrap' \
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
cat > "$SRC_DIR/.wd-source" <<EOF
repo=$REPO
ref=$REF
commit=$SHA
EOF

# --- prefer a binary built by CI -------------------------------------------
# Compiling on the target costs a Rust toolchain, ~2.2 GB of peak memory and a
# build directory on every host. A release binary for this architecture and
# libc family removes all of it. Anything unexpected here is not fatal: the
# function simply declines, and install.sh compiles as it always did.

host_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo x86_64 ;;
    aarch64|arm64) echo aarch64 ;;
    *) echo unsupported ;;
  esac
}

host_family() {
  # Arch has no artifact of its own. Its glibc is newer than either build base,
  # and glibc is backward compatible, so the rhel binary -- built against the
  # oldest glibc of the two -- runs there.
  if command -v dnf >/dev/null 2>&1; then echo rhel
  elif command -v pacman >/dev/null 2>&1; then echo rhel
  elif command -v apt-get >/dev/null 2>&1; then echo debian
  else echo unsupported
  fi
}

# Sets PREBUILT_OK=yes and leaves the verified binary at $2 on success.
try_prebuilt() {
  commit=$1
  dest=$2
  PREBUILT_OK=no

  [ "${WD_PREBUILT:-on}" = off ] && { say "prebuilt binaries disabled; will compile"; return 0; }

  arch=$(host_arch)
  family=$(host_family)
  if [ "$arch" = unsupported ] || [ "$family" = unsupported ]; then
    say "no release build for $(uname -m) on this distro; will compile"
    return 0
  fi

  # A tag installs that tag's release; a branch installs the rolling one.
  case "${WD_RELEASE_TAG:-}" in
    "") case "$REF" in v*) tag=$REF ;; *) tag=latest-main ;; esac ;;
    *)  tag=$WD_RELEASE_TAG ;;
  esac

  base="https://github.com/$REPO/releases/download/$tag"
  asset="webdesk-$arch-$family"
  say "looking for a prebuilt $asset in release $tag"

  # Cache-buster, and it earns its keep on the rolling pointer: latest-main is
  # republished on every push to main, and a cached copy of the previous
  # manifest can outlive it by a minute or so. A host that reads the stale one
  # decides the release belongs to some other commit and compiles for nothing.
  # A unique query string asks for the object as it is now.
  if ! curl -fsSL --max-time 30 -H 'Cache-Control: no-cache' \
       "$base/manifest.json?t=$(date +%s)" -o "$TMP/manifest.json" 2>/dev/null; then
    say "    no release manifest; will compile"
    return 0
  fi

  # Only trust the release if it is the very commit we were about to build.
  built=$(sed -n 's/.*"commit"[[:space:]]*:[[:space:]]*"\([0-9a-f]\{40\}\)".*/\1/p' "$TMP/manifest.json" | head -1)
  if [ -z "$built" ] || [ "$built" != "$commit" ]; then
    say "    release is for ${built:-unknown}, need $commit; will compile"
    return 0
  fi

  # The release number this commit went out as. Recorded alongside the commit so
  # that a later rebuild from this same tree reports the number it was released
  # under, instead of falling back to the floor in Cargo.toml. Only read once the
  # commit above has matched -- a version from some other build would be a lie.
  ver=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$TMP/manifest.json" | head -1)
  if [ -n "$ver" ] && [ -f "$SRC_DIR/.wd-source" ]; then
    printf 'version=%s\n' "$ver" >> "$SRC_DIR/.wd-source"
    say "    release $ver"
  fi

  # From here on, take the bytes from the numbered release the manifest names
  # rather than the rolling pointer we found it through. That release is
  # published once and never rewritten, so unlike latest-main its URLs cannot
  # serve a previous build's bytes no matter what is cached in front of them.
  # A manifest from before releases were numbered carries no tag; those keep
  # using the pointer they came from.
  rel=$(sed -n 's/.*"tag"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$TMP/manifest.json" | head -1)
  case "$rel" in
    v[0-9]*)
      base="https://github.com/$REPO/releases/download/$rel"
      say "    taking assets from $rel"
      ;;
  esac

  if ! curl -fsSL --max-time 300 "$base/$asset" -o "$TMP/$asset" 2>/dev/null; then
    say "    could not download $asset; will compile"
    return 0
  fi
  if ! curl -fsSL --max-time 60 "$base/SHA256SUMS" -o "$TMP/SHA256SUMS" 2>/dev/null; then
    say "    no SHA256SUMS; will compile"
    return 0
  fi

  # Checksum first. This catches a truncated or corrupted download; it is not
  # by itself a provenance control, since the sums come from the same origin as
  # the binary. That is what the attestation below is for.
  if command -v sha256sum >/dev/null 2>&1; then
    got=$(sha256sum "$TMP/$asset" | cut -d" " -f1)
  elif command -v openssl >/dev/null 2>&1; then
    got=$(openssl dgst -sha256 "$TMP/$asset" | sed 's/.*= *//')
  else
    # Declining costs a few minutes of compiling. Installing a binary we could
    # not check costs rather more, so this does not fall through to "install
    # it anyway".
    say "    no sha256sum or openssl to verify with; will compile"
    return 0
  fi

  want=$(sed -n "s/^\([0-9a-f]\{64\}\)  *$asset\$/\1/p" "$TMP/SHA256SUMS" | head -1)
  if [ -z "$want" ] || [ "$want" != "$got" ]; then
    say "    checksum mismatch (wanted ${want:-none}, got $got); will compile"
    return 0
  fi
  say "    checksum ok"

  # Provenance. gh is not a dependency of this project, so its absence only
  # means the check is skipped -- unless the operator asked for it to be
  # mandatory. A gh that is present and says no is always fatal.
  #
  # "Present" has to mean "able to check". gh only grew `attestation` in 2.49,
  # and an older one exits non-zero with `unknown command "attestation"` -- the
  # same way it exits when a binary is genuinely unattested. Reading that as a
  # failed verification condemns every host with an older gh to compiling for
  # ever, which is exactly backwards: a gh that cannot check is not accusing
  # the binary of anything, it is declining to have an opinion. Distinguish the
  # two, and let the operator make either one fatal.
  if ! command -v gh >/dev/null 2>&1; then
    if [ "${WD_REQUIRE_ATTESTATION:-0}" = 1 ]; then
      die "WD_REQUIRE_ATTESTATION=1 but gh is not installed to check it"
    fi
    say "    gh not installed; provenance not checked (see README)"
  elif ! gh attestation --help >/dev/null 2>&1; then
    ghver=$(gh --version 2>/dev/null | sed -n '1s/^gh version \([^ ]*\).*/\1/p')
    if [ "${WD_REQUIRE_ATTESTATION:-0}" = 1 ]; then
      die "WD_REQUIRE_ATTESTATION=1 but gh ${ghver:-(unknown version)} cannot verify attestations; needs 2.49 or newer"
    fi
    say "    gh ${ghver:-here} is too old to verify provenance (needs 2.49+); not checked"
  elif gh attestation verify "$TMP/$asset" --repo "$REPO" >/dev/null 2>&1; then
    say "    provenance verified against $REPO"
  else
    say "!! provenance verification FAILED for $asset"
    [ "${WD_REQUIRE_ATTESTATION:-0}" = 1 ] && die "refusing to install an unverified binary"
    say "    refusing the prebuilt binary; will compile instead"
    return 0
  fi

  mkdir -p "$(dirname "$dest")"
  cp "$TMP/$asset" "$dest"
  chmod 0755 "$dest"
  PREBUILT_OK=yes
  say "    installed prebuilt binary, skipping the build"
  return 0
}

# --- get a binary -----------------------------------------------------------
# Drop any binary left by a previous run before deciding. install.sh installs
# whatever is sitting at this path, so a stale one here would be reinstalled as
# though it were the new version -- the reason the updater used to have to pass
# FORCE_BUILD=1.
BIN_PATH="$SRC_DIR/target/release/webdesk"
rm -f "$BIN_PATH"

if [ -n "$SHA" ]; then
  try_prebuilt "$SHA" "$BIN_PATH"
else
  say "ref was not resolved to a commit; will compile"
fi

# --- build and install ------------------------------------------------------
# Explicitly, because `exec` replaces this process and the EXIT trap never runs.
# Without this every install and every update leaves its tarball and unpacked
# source tree in /tmp forever -- measured at 6.1 MB after nine runs on the test
# host, and it only ever grows.
rm -rf "$TMP"
trap - EXIT INT TERM

say "handing over to install.sh"
# Exported rather than passed as a prefix to `exec`: assignments in front of a
# special built-in are not reliably placed in the new image's environment, and
# install.sh silently falling back to the default ref would be a quiet way to
# install one thing and track another.
export WD_REPO=$REPO
export WD_REF=$REF
export WD_SRC_DIR=$SRC_DIR
exec bash "$SRC_DIR/install.sh"
