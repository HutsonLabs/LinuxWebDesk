#!/usr/bin/env bash
# Copy this tree to a host and run install.sh there.
#   ./deploy.sh 10.1.2.40           (uses your current username)
#   ./deploy.sh user@10.1.2.40
#
# For a host you do not already have a checkout for, bootstrap.sh is the
# shorter path -- see the README.
set -euo pipefail
HOST=${1:?usage: ./deploy.sh [user@]host}
DIR=${2:-/tmp/webdesk}

# Record what is being shipped. The tree arrives without .git (rsync excludes
# it), so without this the installed binary could not say which commit it is,
# and the update check would have nothing to compare against.
SHA=$(git rev-parse HEAD 2>/dev/null || echo "")
if [ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]; then
  SHA="${SHA}-dirty"
fi
REF=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo main)
REPO=$(git remote get-url origin 2>/dev/null \
       | sed -e 's#^git@github.com:#https://github.com/#' \
             -e 's#^https://github.com/##' -e 's#\.git$##' \
       || echo HutsonLabs/WebDesk)

cat > .wd-source <<EOF
repo=$REPO
ref=$REF
commit=$SHA
EOF

echo "==> syncing to $HOST:$DIR ($REF @ ${SHA:-unknown})"
rsync -az --delete \
  --exclude target/ --exclude .git/ --exclude docs/ \
  ./ "$HOST:$DIR/"

echo "==> building and installing (sudo on remote)"
ssh -t "$HOST" "cd $DIR && sudo bash install.sh"
