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

need_root() { [ "$(id -u)" -eq 0 ] || { echo "run as root (sudo $0)"; exit 1; }; }
need_root

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
