#!/usr/bin/env bash
# Copy this tree to a host and run install.sh there.
#   ./deploy.sh 10.1.2.40           (uses your current username)
#   ./deploy.sh user@10.1.2.40
#
# Settings go through the environment, the same names install.sh reads:
#   PORT=61443 ./deploy.sh 10.1.2.40
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

# install.sh reads its settings from the environment, but it runs at the far
# end of an ssh, where this shell's environment does not reach -- so without
# forwarding, `PORT=61443 ./deploy.sh host` would silently install on whatever
# the host recorded last time. Only names that are actually set are passed on:
# an unset one has to stay unset over there, or it would override the value
# install.conf remembered with a default the caller never asked for.
#
# FORCE_BUILD is on by default here, unlike the other names. install.sh prefers
# an existing target/release/webdesk so that a local build is not repeated as
# root -- but this path excludes target/ from the sync, and rsync protects an
# excluded directory from --delete, so whatever that tree already holds is a
# binary from an *earlier* deploy. Preferring it would ship the new sources and
# then run the old code. Set FORCE_BUILD=0 to opt back out -- install.sh only
# tests whether the name is non-empty, so opting out means not forwarding it.
FORCE_BUILD=${FORCE_BUILD:-1}
case $FORCE_BUILD in 0|no|off) FORCE_BUILD="" ;; esac
ENV_ARGS=()
for _v in PREFIX PORT CONF_DIR FORCE_BUILD WD_REPO WD_REF WD_SRC_DIR WD_STATE_DIR \
          WD_ADMIN_GROUPS WD_UPDATE WD_TLS WD_TLS_CERT WD_TLS_KEY; do
  if [ -n "${!_v:-}" ]; then ENV_ARGS+=("$_v=${!_v}"); fi
done
ENV_PREFIX=""
if [ ${#ENV_ARGS[@]} -gt 0 ]; then
  ENV_PREFIX=$(printf '%q ' "${ENV_ARGS[@]}")
  echo "==> settings: ${ENV_ARGS[*]}"
fi

echo "==> syncing to $HOST:$DIR ($REF @ ${SHA:-unknown})"
rsync -az --delete \
  --exclude target/ --exclude .git/ --exclude docs/ \
  ./ "$HOST:$DIR/"

echo "==> building and installing (sudo on remote)"
ssh -t "$HOST" "cd $DIR && sudo ${ENV_PREFIX}bash install.sh"
