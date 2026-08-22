#!/usr/bin/env bash
# Runs ON the target host. Installs build dependencies, builds, and starts the
# service. Safe to re-run -- it is the upgrade path too.
set -euo pipefail

PREFIX=${PREFIX:-/usr/local/bin}
PORT=${PORT:-7788}
SRC=$(cd "$(dirname "$0")" && pwd)

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
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable --now linuxwebdesk
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
