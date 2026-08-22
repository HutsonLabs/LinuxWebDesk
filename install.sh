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
    # libpam0g-dev for PAM headers, clang because pam-client uses bindgen.
    apt-get install -y -qq build-essential pkg-config libpam0g-dev clang curl >/dev/null
    ;;
  rhel)
    dnf install -y -q gcc gcc-c++ make pkgconf-pkg-config pam-devel clang curl >/dev/null
    ;;
esac

echo "==> ensuring a rust toolchain"
if ! command -v cargo >/dev/null 2>&1; then
  export CARGO_HOME=/opt/rust RUSTUP_HOME=/opt/rust
  curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal >/dev/null
fi
export PATH="/opt/rust/bin:$HOME/.cargo/bin:$PATH"
cargo --version

echo "==> building (release)"
cd "$SRC"
cargo build --release

echo "==> installing binary"
install -m 0755 target/release/rockywebde "$PREFIX/rockywebde"

echo "==> installing PAM service"
# The stack differs between families; include whichever this host provides so
# local accounts, SSSD and LDAP all resolve through the host's own policy.
if [ -f /etc/pam.d/system-auth ]; then
  cat > /etc/pam.d/rockywebde <<'PAM'
auth       include      system-auth
account    include      system-auth
PAM
elif [ -f /etc/pam.d/common-auth ]; then
  cat > /etc/pam.d/rockywebde <<'PAM'
@include common-auth
@include common-account
PAM
else
  echo "!! no system-auth or common-auth found; write /etc/pam.d/rockywebde yourself"; exit 1
fi
chmod 0644 /etc/pam.d/rockywebde

echo "==> installing systemd unit"
cat > /etc/systemd/system/rockywebde.service <<UNIT
[Unit]
Description=RockyWebDE
After=network-online.target
Wants=network-online.target

[Service]
# Root is required to authenticate through PAM and to drop to the logged-in
# user. Every filesystem operation runs in an unprivileged child instead.
ExecStart=$PREFIX/rockywebde
Environment=RWDE_LISTEN=0.0.0.0:$PORT
Environment=RUST_LOG=rockywebde=info
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable --now rockywebde
sleep 1
systemctl --no-pager --lines=15 status rockywebde || true

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
