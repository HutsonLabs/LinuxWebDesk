#!/usr/bin/env bash
# Runs ON the target host. Installs build dependencies, builds, and starts the
# service. Safe to re-run -- it is the upgrade path too.
set -euo pipefail

SRC=$(cd "$(dirname "$0")" && pwd)

# An update re-runs this script with no arguments, so anything chosen at first
# install has to be remembered or it would quietly revert to the defaults --
# a host installed on port 9000 must not come back on 6767.
#
# Precedence is: what the caller passed, then what was recorded last time, then
# the default. Sourcing the conf file assigns the same WD_* names the caller
# may have exported, so the incoming values have to be taken down first --
# otherwise `WD_REF=v0.2.0 bootstrap.sh` would install v0.2.0 and then record
# and track `main`.
_in_prefix=${PREFIX:-}
_in_port=${PORT:-}
_in_repo=${WD_REPO:-}
_in_ref=${WD_REF:-}
_in_src_dir=${WD_SRC_DIR:-}
_in_state_dir=${WD_STATE_DIR:-}
_in_admin=${WD_ADMIN_GROUPS:-}
_in_updates=${WD_UPDATE:-}
_in_tls=${WD_TLS:-}
_in_tls_cert=${WD_TLS_CERT:-}
_in_tls_key=${WD_TLS_KEY:-}
_in_apps=${WD_APPS:-}

CONF_DIR=${CONF_DIR:-/etc/webdesk}
CONF=$CONF_DIR/install.conf
# shellcheck source=/dev/null
[ -r "$CONF" ] && . "$CONF"

PREFIX=${_in_prefix:-${WD_PREFIX:-/usr/local/bin}}
# 61443 rather than 6767, and https rather than http. This asks for a password
# and hands back a shell; plaintext was never the right default. A host that
# was installed on the old port keeps it -- WD_PORT is remembered above -- and
# only changes scheme.
PORT=${_in_port:-${WD_PORT:-61443}}
# Anything but `off`, which is for a host with a reverse proxy already
# terminating TLS in front of this one.
TLS=${_in_tls:-${WD_TLS:-on}}
TLS_CERT=${_in_tls_cert:-${WD_TLS_CERT:-}}
TLS_KEY=${_in_tls_key:-${WD_TLS_KEY:-}}
STATE_DIR=${_in_state_dir:-${WD_STATE_DIR:-/var/lib/webdesk}}
LIBEXEC=${LIBEXEC:-/usr/local/libexec}
REPO=${_in_repo:-${WD_REPO:-HutsonLabs/WebDesk}}
REF=${_in_ref:-${WD_REF:-main}}
SRC_DIR=${_in_src_dir:-${WD_SRC_DIR:-/usr/local/src/webdesk}}
ADMIN_GROUPS=${_in_admin:-${WD_ADMIN_GROUPS:-wheel,sudo}}
UPDATES=${_in_updates:-${WD_UPDATE:-on}}
# Which kinds of app this host should be provisioned to run, as a comma list of
# the group names src/deps.rs reports -- containers, streamed, host -- or `all`.
#
# Empty by default, and that default is the point. A WebDesk install adds
# WebDesk: a binary, a unit, a PAM file and this conf. A host that will only
# ever want the file manager and the terminal has no business carrying a
# container engine, a compositor or a Cockpit bridge because the installer
# assumed it might. Anything named here is installed once, at install time; the
# rest is reported at the end and offered in the Apps window, where it is one
# press for a member of $ADMIN_GROUPS.
#
# The words are the words the API uses, not a second vocabulary for the same
# three things -- `desktops` was the obvious alternative and is the wrong one,
# because catalog.rs already spells the *container* desktop entries with it.
APPS=${_in_apps:-${WD_APPS:-}}

need_root() { [ "$(id -u)" -eq 0 ] || { echo "run as root (sudo $0)"; exit 1; }; }
need_root

ALL_GROUPS="containers streamed host"

# Checked here, before anything is downloaded or built, because the failure this
# prevents is silent: `WD_APPS=desktops` is not an error anywhere downstream, it
# simply matches no group, installs nothing, and leaves an operator who asked
# for a container engine looking at an Apps window that says there is none.
#
# Named APP_GROUPS and not GROUPS: bash owns GROUPS, keeps the invoking user's
# gids in it, and quietly discards an assignment to it. The first version of
# this read `WD_APPS=all` and then provisioned group "1000", which is not a
# group and would have exited on the operator's own gid.
APP_GROUPS=""
case $APPS in
  ""|none) ;;
  all) APP_GROUPS=$ALL_GROUPS ;;
  *)
    for g in $(echo "$APPS" | tr ',' ' '); do
      case " $ALL_GROUPS " in
        *" $g "*) APP_GROUPS="$APP_GROUPS $g" ;;
        *) echo "WD_APPS: '$g' is not a group. Choose from: $ALL_GROUPS, or all"; exit 1 ;;
      esac
    done
    ;;
esac

echo "==> detecting distribution"
if command -v apt-get >/dev/null 2>&1; then
  FAMILY=debian
elif command -v dnf >/dev/null 2>&1; then
  FAMILY=rhel
elif command -v pacman >/dev/null 2>&1; then
  FAMILY=arch
else
  echo "unsupported: need apt-get, dnf or pacman"; exit 1
fi
echo "    family: $FAMILY"

echo "==> installing build dependencies"
case $FAMILY in
  debian)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    # No PAM headers or clang needed: the FFI is hand-written and build.rs
    # links libpam.so.0 directly.
    apt-get install -y -qq gcc make curl >/dev/null
    ;;
  rhel)
    # Minimal RHEL-family images ship curl-minimal, which conflicts with the
    # curl package -- asking for curl outright fails the whole transaction and
    # takes the install with it. curl-minimal already provides the binary, so
    # only install curl when the host genuinely has none.
    dnf install -y -q gcc make >/dev/null
    command -v curl >/dev/null 2>&1 || dnf install -y -q --allowerasing curl >/dev/null
    ;;
  arch)
    # --needed so a rolling system is not churned on every re-run, and no -Syu:
    # a partial upgrade is how you break an Arch box, and a full one is not
    # this installer's decision to make.
    pacman -Sy --needed --noconfirm gcc make curl >/dev/null
    ;;
esac

# Under sudo, $HOME becomes /root. If we borrow the invoking user's toolchain we
# must borrow RUSTUP_HOME with it, or the cargo shim finds no default toolchain.
if [ -n "${SUDO_USER:-}" ]; then
  _uh=$(getent passwd "$SUDO_USER" | cut -d: -f6 || true)
  if [ -x "$_uh/.cargo/bin/cargo" ]; then
    export PATH="$_uh/.cargo/bin:$PATH"
    export RUSTUP_HOME="${RUSTUP_HOME:-$_uh/.rustup}"
    export CARGO_HOME="${CARGO_HOME:-$_uh/.cargo}"
  fi
fi

# A previous run may have put a toolchain in /opt/rust. The updater is started
# by systemd with a minimal PATH and no SUDO_USER, so it would not otherwise
# find it and would reinstall rustup on every update.
if ! command -v cargo >/dev/null 2>&1 && [ -x /opt/rust/bin/cargo ]; then
  export PATH="/opt/rust/bin:$PATH"
  export CARGO_HOME="${CARGO_HOME:-/opt/rust}" RUSTUP_HOME="${RUSTUP_HOME:-/opt/rust}"
fi

BIN="$SRC/target/release/webdesk"

if [ -x "$BIN" ] && [ -z "${FORCE_BUILD:-}" ]; then
  # Prefer a binary that was already built unprivileged. Building as root would
  # otherwise scatter root-owned files through the invoking user's cargo cache.
  echo "==> using prebuilt binary ($(stat -c%s "$BIN") bytes)"
  echo "    set FORCE_BUILD=1 to rebuild instead"
else
  echo "==> ensuring a rust toolchain"
  if ! command -v cargo >/dev/null 2>&1; then
    export CARGO_HOME=/opt/rust RUSTUP_HOME=/opt/rust
    curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal >/dev/null
    export PATH="/opt/rust/bin:$PATH"
  fi
  cargo --version

  echo "==> building (release)"
  cd "$SRC"
  cargo build --release
fi

echo "==> installing binary"
install -m 0755 "$BIN" "$PREFIX/webdesk"

echo "==> installing the updater"
install -d -m 0700 "$STATE_DIR"
if [ -f "$SRC/libexec/webdesk-update" ]; then
  install -D -m 0755 "$SRC/libexec/webdesk-update" "$LIBEXEC/webdesk-update"
  ln -sf "$LIBEXEC/webdesk-update" "$PREFIX/webdesk-update"
  echo "    $LIBEXEC/webdesk-update (also on PATH as webdesk-update)"
else
  echo "    !! libexec/webdesk-update missing from this tree; in-browser"
  echo "       updates will report themselves unavailable"
fi

echo "==> recording install settings in $CONF"
install -d -m 0755 "$CONF_DIR"
cat > "$CONF" <<CONFEOF
# Written by install.sh. Read back by install.sh and webdesk-update.
WD_REPO=$REPO
WD_REF=$REF
WD_SRC_DIR=$SRC_DIR
WD_PREFIX=$PREFIX
WD_PORT=$PORT
WD_STATE_DIR=$STATE_DIR
WD_ADMIN_GROUPS=$ADMIN_GROUPS
WD_UPDATE=$UPDATES
WD_TLS=$TLS
WD_TLS_CERT=$TLS_CERT
WD_TLS_KEY=$TLS_KEY
WD_APPS=$APPS
CONFEOF
chmod 0644 "$CONF"

echo "==> installing PAM service"
# The stack differs between families; include whichever this host provides so
# local accounts, SSSD and LDAP all resolve through the host's own policy.
if [ -f /etc/pam.d/system-auth ]; then
  cat > /etc/pam.d/webdesk <<'PAM'
auth       include      system-auth
account    include      system-auth
PAM
elif [ -f /etc/pam.d/common-auth ]; then
  cat > /etc/pam.d/webdesk <<'PAM'
@include common-auth
@include common-account
PAM
else
  echo "!! no system-auth or common-auth found; write /etc/pam.d/webdesk yourself"; exit 1
fi
chmod 0644 /etc/pam.d/webdesk

echo "==> installing systemd unit"
cat > /etc/systemd/system/webdesk.service <<UNIT
[Unit]
Description=WebDesk
After=network-online.target
Wants=network-online.target

[Service]
# Root is required to authenticate through PAM and to drop to the logged-in
# user. Every filesystem operation runs in an unprivileged child instead.
ExecStart=$PREFIX/webdesk
Environment=WD_LISTEN=0.0.0.0:$PORT
Environment=RUST_LOG=webdesk=info
# TLS. With no certificate named, webdesk writes a self-signed one into
# $STATE_DIR/tls on first start and keeps using it -- which is why the browser
# asks about it once per host. Set WD_TLS_CERT and WD_TLS_KEY to PEM paths for
# a real one, or WD_TLS=off if something in front already terminates TLS.
Environment=WD_TLS=$TLS
Environment=WD_TLS_CERT=$TLS_CERT
Environment=WD_TLS_KEY=$TLS_KEY
# Self-update. WD_ADMIN_GROUPS decides who may trigger one -- membership is
# resolved through NSS, so it means whatever it means to sudo on this host.
# Set WD_UPDATE=off to remove the capability entirely.
Environment=WD_UPDATE=$UPDATES
Environment=WD_ADMIN_GROUPS=$ADMIN_GROUPS
Environment=WD_STATE_DIR=$STATE_DIR
Environment=WD_UPDATER=$LIBEXEC/webdesk-update
Environment=WD_REPO=$REPO
Environment=WD_REF=$REF
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable webdesk >/dev/null 2>&1 || true
# `enable --now` only starts a *stopped* unit. On an upgrade the service is
# already running, so it would keep executing the old binary from the inode it
# already has open, and the install would look like it had done nothing.
systemctl restart webdesk
sleep 1
systemctl --no-pager --lines=15 status webdesk || true

echo "==> firewall"
if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --add-port=$PORT/tcp --permanent >/dev/null && firewall-cmd --reload >/dev/null
  echo "    opened $PORT/tcp (firewalld)"
# `grep -q active` matched the word inside "Status: inactive", so this branch
# fired on every host with ufw installed and switched off -- adding a rule to a
# firewall that is not running and reporting a port had been opened when
# nothing had. Anchor on the whole status line.
elif command -v ufw >/dev/null 2>&1 && ufw status | grep -q '^Status: active'; then
  ufw allow "$PORT"/tcp >/dev/null && echo "    opened $PORT/tcp (ufw)"
else
  echo "    no active firewall detected; nothing to open"
fi

# ---------------------------------------------------------------- app runtimes
#
# What a group costs on this family, as package names. This duplicates the table
# in src/deps.rs, and it has to: the binary that owns that table is not built yet
# when this runs, and on a cold host it will not be for another few minutes. So
# the copy is made cheap to check instead -- a test in src/deps.rs reads these
# very lines and fails the build if the two ever disagree about a name, or about
# whether a group can be provisioned here at all. Which means the shape of these
# arms is load-bearing: one arm per line, and the packages in double quotes.
#
# An empty answer is not an omission. It means this project has no honest package
# name for that group on this family, and the summary below says which and why
# rather than running a package manager against a guess.
wd_group_packages() {
  case "$1:$2" in
    # One engine, and the same engine everywhere. Podman is in the base
    # repositories of all three families under one name; Docker is only in
    # Debian's and Arch's, and on the RHEL family it means adding Docker's own
    # repository and key, which is an operator's decision and not an installer's.
    # Installing docker.io here and podman there would make `WD_APPS=containers`
    # mean two different machines, which is worse than choosing one and saying so
    # -- and the README still calls Podman accepted but untested, which the
    # summary below repeats where somebody will read it.
    containers:debian) echo "podman" ;;
    containers:rhel)   echo "podman" ;;
    containers:arch)   echo "podman" ;;
    streamed:debian)   echo "flatpak cage wayvnc" ;;
    streamed:arch)     echo "flatpak cage wayvnc" ;;
    # `dnf` alone does not say enough here, and this script has not got enough to
    # find out with. On Fedora cage and wayvnc are in the base repositories; on
    # Enterprise Linux they are in EPEL, which is a third-party repository this
    # installer will not enable on somebody's host -- and on EL9 there is no cage
    # build at any version, so there is nothing to enable EPEL *for*. Flatpak is
    # the part that is right on all three, so that is the part that is installed;
    # the summary below reads /etc/os-release and says which of the three this is,
    # and src/deps.rs makes the rest one press for whoever is looking at it.
    streamed:rhel)     echo "flatpak" ;;
    host:debian)       echo "cockpit-bridge" ;;
    host:rhel)         echo "cockpit-bridge" ;;
    # Arch has no bridge package apart from `cockpit`, which is all of Cockpit
    # including the web server. The Apps window offers it anyway, with the cost in
    # the sentence beside the button -- but a comma in WD_APPS is not somebody
    # reading that sentence, so this installs nothing and the summary says why.
    host:arch)         echo "" ;;
    *) echo "" ;;
  esac
}

if [ -n "$APP_GROUPS" ]; then
  echo "==> provisioning app runtimes ($(echo "$APP_GROUPS" | tr -s ' ' | sed 's/^ //;s/ /, /g'))"
  WANT=""
  for g in $APP_GROUPS; do
    WANT="$WANT $(wd_group_packages "$g" "$FAMILY")"
  done
  # shellcheck disable=SC2086
  WANT=$(echo $WANT | tr ' ' '\n' | sort -u | tr '\n' ' ')
  if [ -z "$(echo "$WANT" | tr -d ' ')" ]; then
    echo "    nothing this installer can name on $FAMILY; see the summary below"
  else
    echo "    $WANT"
    # Not fatal. WebDesk itself is already installed and running by this point,
    # and a host that cannot reach a mirror should end up with a working desk and
    # a note about what is missing, not with `set -e` killing the script two
    # lines before it would have told anybody anything.
    # $WANT is a list and has to split into separate arguments; every word in it
    # came from the table above and never from the caller.
    case $FAMILY in
      debian)
        # shellcheck disable=SC2086
        apt-get install -y -qq $WANT || echo "    !! some packages did not install" ;;
      rhel)
        # shellcheck disable=SC2086
        dnf install -y -q $WANT || echo "    !! some packages did not install" ;;
      arch)
        # shellcheck disable=SC2086
        pacman -Sy --needed --noconfirm $WANT || echo "    !! some packages did not install" ;;
    esac
  fi
fi

# What is still missing and what it costs, whether or not anything was asked for
# -- an operator who installed with the default wants this most of all, and one
# who asked for a group wants to know it did not arrive.
#
# Probed by binary rather than by asking the package manager: the binary is what
# a compositor or an engine actually needs, it is spelled the same everywhere,
# and a host where somebody built one from source has it however the package
# database feels about it.
have() { command -v "$1" >/dev/null 2>&1; }

# One field out of /etc/os-release, without sourcing it -- that file sets NAME,
# VERSION and half a dozen other common words, and this script has its own.
os_field() { sed -n "s/^$1=\"\\?\\([^\"]*\\)\"\\?\$/\\1/p" /etc/os-release 2>/dev/null | head -1; }
# The Enterprise Linux generation, or 0 for Fedora. Only asked once FAMILY is
# rhel, because that is the one place `dnf` turns out to be three package
# universes and the generation is what tells them apart.
el_major() {
  [ "$(os_field ID)" = fedora ] && { echo 0; return; }
  m=$(os_field VERSION_ID | cut -d. -f1)
  case $m in ''|*[!0-9]*) echo 0 ;; *) echo "$m" ;; esac
}

MISSING=""
have docker || have podman || MISSING="$MISSING containers"
{ have flatpak && have cage && have wayvnc; } || MISSING="$MISSING streamed"
have cockpit-bridge || MISSING="$MISSING host"

if [ -n "$MISSING" ]; then
  echo
  echo "this host is not yet set up to run some kinds of app:"
  for g in $MISSING; do
    case $g in
      containers)
        echo "  - no container engine (docker or podman). The desktop entries and the"
        echo "    editor will not install until there is one." ;;
      streamed)
        LACK=
        have flatpak || LACK="$LACK flatpak"
        have cage || LACK="$LACK cage"
        have wayvnc || LACK="$LACK wayvnc"
        printf '  - missing%s. Apps that run on this host and are streamed\n' "$LACK"
        echo "    into the browser will not install until all three are here."
        if [ "$FAMILY" = rhel ] && ! have cage; then
          if [ "$(os_field ID)" = fedora ]; then
            echo "    Fedora has both in its base repositories; the Apps window will install"
            echo "    them in one press."
          elif [ "$(el_major)" -ge 10 ]; then
            echo "    Both come from EPEL here, not from any base repository. Enable EPEL --"
            echo "    which WebDesk will not do to your host for you -- and the Apps window"
            echo "    will install them in one press."
          else
            echo "    EPEL never built cage for Enterprise Linux $(el_major); it starts at 10. There is"
            echo "    no compositor package to install under any name, so streamed apps are"
            echo "    not available on this release. Container apps and the host panels are."
          fi
        fi ;;
      host)
        echo "  - no cockpit-bridge. The Services, Logs and Metrics panels will stay empty."
        if [ "$FAMILY" = arch ]; then
          echo "    Arch has no bridge package apart from \`cockpit\`, which is all of Cockpit"
          echo "    including its web server. The Apps window will offer it, with that cost"
          echo "    stated; this installer will not make that choice from a comma in WD_APPS."
        fi ;;
    esac
  done
  echo
  RERUN=$(echo "$MISSING" | tr -s ' ' | sed 's/^ //;s/ /,/g')
  echo "none of this needs a shell: sign in, open Apps, and anyone in $ADMIN_GROUPS can"
  echo "install any of it in one press. Or re-run this installer with WD_APPS=$RERUN."
fi

echo
if [ "$TLS" = off ]; then
  SCHEME=http
else
  SCHEME=https
  if [ -z "$TLS_CERT" ]; then
    echo "note: serving a self-signed certificate from $STATE_DIR/tls."
    echo "      the browser will ask about it once per host. point WD_TLS_CERT"
    echo "      and WD_TLS_KEY at a real pair to stop it asking."
  fi
fi
echo "ready -> $SCHEME://$(hostname -I 2>/dev/null | awk '{print $1}'):$PORT"
