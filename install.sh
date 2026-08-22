#!/usr/bin/env bash
# Runs ON the target host. Installs build dependencies, builds, and starts the
# service. Safe to re-run -- it is the upgrade path too.
set -euo pipefail

SRC=$(cd "$(dirname "$0")" && pwd)

# An update re-runs this script with no arguments, so anything chosen at first
# install has to be remembered or it would quietly revert to the defaults --
# a host installed on port 9000 must not come back on 7788.
#
# Precedence is: what the caller passed, then what was recorded last time, then
# the default. Sourcing the conf file assigns the same LWD_* names the caller
# may have exported, so the incoming values have to be taken down first --
# otherwise `LWD_REF=v0.2.0 bootstrap.sh` would install v0.2.0 and then record
# and track `main`.
_in_prefix=${PREFIX:-}
_in_port=${PORT:-}
_in_repo=${LWD_REPO:-}
_in_ref=${LWD_REF:-}
_in_src_dir=${LWD_SRC_DIR:-}
_in_state_dir=${LWD_STATE_DIR:-}
_in_admin=${LWD_ADMIN_GROUPS:-}
_in_updates=${LWD_UPDATE:-}

CONF_DIR=${CONF_DIR:-/etc/linuxwebdesk}
CONF=$CONF_DIR/install.conf
# shellcheck source=/dev/null
[ -r "$CONF" ] && . "$CONF"

PREFIX=${_in_prefix:-${LWD_PREFIX:-/usr/local/bin}}
PORT=${_in_port:-${LWD_PORT:-7788}}
STATE_DIR=${_in_state_dir:-${LWD_STATE_DIR:-/var/lib/linuxwebdesk}}
LIBEXEC=${LIBEXEC:-/usr/local/libexec}
REPO=${_in_repo:-${LWD_REPO:-HutsonLabs/LinuxWebDesk}}
REF=${_in_ref:-${LWD_REF:-main}}
SRC_DIR=${_in_src_dir:-${LWD_SRC_DIR:-/usr/local/src/linuxwebdesk}}
ADMIN_GROUPS=${_in_admin:-${LWD_ADMIN_GROUPS:-wheel,sudo}}
UPDATES=${_in_updates:-${LWD_UPDATE:-on}}

need_root() { [ "$(id -u)" -eq 0 ] || { echo "run as root (sudo $0)"; exit 1; }; }
need_root

echo "==> detecting distribution"
if command -v apt-get >/dev/null 2>&1; then
  FAMILY=debian
elif command -v dnf >/dev/null 2>&1; then
  FAMILY=rhel
else
  echo "unsupported: need apt-get or dnf"; exit 1
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
    dnf install -y -q gcc make curl >/dev/null
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

BIN="$SRC/target/release/linuxwebdesk"

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

# This project was called "rockywebde" before the rename. On a host that ran
# that version the old unit is still enabled and still holds $PORT, so the new
# one would fail to bind. Retire the old install before putting the new one in.
OLD=rockywebde
if [ -e /etc/systemd/system/$OLD.service ] || [ -x "$PREFIX/$OLD" ]; then
  echo "==> migrating from $OLD"
  systemctl disable --now $OLD >/dev/null 2>&1 || true
  rm -f /etc/systemd/system/$OLD.service /etc/pam.d/$OLD "$PREFIX/$OLD"
  systemctl daemon-reload
  echo "    removed the old unit, PAM service file and binary"
fi

echo "==> installing binary"
install -m 0755 "$BIN" "$PREFIX/linuxwebdesk"

echo "==> installing the updater"
install -d -m 0700 "$STATE_DIR"
if [ -f "$SRC/libexec/linuxwebdesk-update" ]; then
  install -D -m 0755 "$SRC/libexec/linuxwebdesk-update" "$LIBEXEC/linuxwebdesk-update"
  ln -sf "$LIBEXEC/linuxwebdesk-update" "$PREFIX/linuxwebdesk-update"
  echo "    $LIBEXEC/linuxwebdesk-update (also on PATH as linuxwebdesk-update)"
else
  echo "    !! libexec/linuxwebdesk-update missing from this tree; in-browser"
  echo "       updates will report themselves unavailable"
fi

echo "==> recording install settings in $CONF"
install -d -m 0755 "$CONF_DIR"
cat > "$CONF" <<CONFEOF
# Written by install.sh. Read back by install.sh and linuxwebdesk-update.
LWD_REPO=$REPO
LWD_REF=$REF
LWD_SRC_DIR=$SRC_DIR
LWD_PREFIX=$PREFIX
LWD_PORT=$PORT
LWD_STATE_DIR=$STATE_DIR
LWD_ADMIN_GROUPS=$ADMIN_GROUPS
LWD_UPDATE=$UPDATES
CONFEOF
chmod 0644 "$CONF"

echo "==> installing PAM service"
# The stack differs between families; include whichever this host provides so
# local accounts, SSSD and LDAP all resolve through the host's own policy.
if [ -f /etc/pam.d/system-auth ]; then
  cat > /etc/pam.d/linuxwebdesk <<'PAM'
auth       include      system-auth
account    include      system-auth
PAM
elif [ -f /etc/pam.d/common-auth ]; then
  cat > /etc/pam.d/linuxwebdesk <<'PAM'
@include common-auth
@include common-account
PAM
else
  echo "!! no system-auth or common-auth found; write /etc/pam.d/linuxwebdesk yourself"; exit 1
fi
chmod 0644 /etc/pam.d/linuxwebdesk

echo "==> installing systemd unit"
cat > /etc/systemd/system/linuxwebdesk.service <<UNIT
[Unit]
Description=LinuxWebDesk
After=network-online.target
Wants=network-online.target

[Service]
# Root is required to authenticate through PAM and to drop to the logged-in
# user. Every filesystem operation runs in an unprivileged child instead.
ExecStart=$PREFIX/linuxwebdesk
Environment=LWD_LISTEN=0.0.0.0:$PORT
Environment=RUST_LOG=linuxwebdesk=info
# Self-update. LWD_ADMIN_GROUPS decides who may trigger one -- membership is
# resolved through NSS, so it means whatever it means to sudo on this host.
# Set LWD_UPDATE=off to remove the capability entirely.
Environment=LWD_UPDATE=$UPDATES
Environment=LWD_ADMIN_GROUPS=$ADMIN_GROUPS
Environment=LWD_STATE_DIR=$STATE_DIR
Environment=LWD_UPDATER=$LIBEXEC/linuxwebdesk-update
Environment=LWD_REPO=$REPO
Environment=LWD_REF=$REF
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable linuxwebdesk >/dev/null 2>&1 || true
# `enable --now` only starts a *stopped* unit. On an upgrade the service is
# already running, so it would keep executing the old binary from the inode it
# already has open, and the install would look like it had done nothing.
systemctl restart linuxwebdesk
sleep 1
systemctl --no-pager --lines=15 status linuxwebdesk || true

echo "==> firewall"
if command -v firewall-cmd >/dev/null 2>&1 && firewall-cmd --state >/dev/null 2>&1; then
  firewall-cmd --add-port=$PORT/tcp --permanent >/dev/null && firewall-cmd --reload >/dev/null
  echo "    opened $PORT/tcp (firewalld)"
elif command -v ufw >/dev/null 2>&1 && ufw status | grep -q active; then
  ufw allow "$PORT"/tcp >/dev/null && echo "    opened $PORT/tcp (ufw)"
else
  echo "    no active firewall detected; nothing to open"
fi

echo
echo "ready -> http://$(hostname -I 2>/dev/null | awk '{print $1}'):$PORT"
