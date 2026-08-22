#!/usr/bin/env bash
# Copy this tree to a host and run install.sh there.
#   ./deploy.sh 10.1.2.40           (uses your current username)
#   ./deploy.sh user@10.1.2.40
set -euo pipefail
HOST=${1:?usage: ./deploy.sh [user@]host}
DIR=${2:-/tmp/linuxwebdesk}

echo "==> syncing to $HOST:$DIR"
rsync -az --delete \
  --exclude target/ --exclude .git/ --exclude docs/ \
  ./ "$HOST:$DIR/"

echo "==> building and installing (sudo on remote)"
ssh -t "$HOST" "cd $DIR && sudo bash install.sh"
