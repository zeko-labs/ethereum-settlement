#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/tools/lib/workspace.sh"

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

make_repo() {
  local path=$1 marker=$2
  mkdir -p "$path/.git" "$path/$marker"
}

WORKSPACE=$TMP_DIR/arbitrary-parent
SETTLEMENT_ROOT=$WORKSPACE/ethereum-settlement
make_repo "$SETTLEMENT_ROOT" tools
make_repo "$WORKSPACE/zeko" src/app/zeko
make_repo "$WORKSPACE/zeko-ui" packages/eth-bridge-sdk

unset ZEKO_WORKSPACE_ROOT ZEKO_ROOT ZEKO_UI_ROOT
zeko_resolve_companion_repo "$SETTLEMENT_ROOT" ZEKO_ROOT zeko src/app/zeko
zeko_resolve_companion_repo "$SETTLEMENT_ROOT" ZEKO_UI_ROOT zeko-ui packages/eth-bridge-sdk
[[ $ZEKO_ROOT == "$WORKSPACE/zeko" ]]
[[ $ZEKO_UI_ROOT == "$WORKSPACE/zeko-ui" ]]

OVERRIDE_ROOT=$TMP_DIR/override
make_repo "$OVERRIDE_ROOT/core" src/app/zeko
make_repo "$OVERRIDE_ROOT/web" packages/eth-bridge-sdk
ZEKO_WORKSPACE_ROOT=$OVERRIDE_ROOT
ZEKO_ROOT=core
ZEKO_UI_ROOT=$OVERRIDE_ROOT/web
zeko_resolve_companion_repo "$SETTLEMENT_ROOT" ZEKO_ROOT zeko src/app/zeko
zeko_resolve_companion_repo "$SETTLEMENT_ROOT" ZEKO_UI_ROOT zeko-ui packages/eth-bridge-sdk
[[ $ZEKO_ROOT == "$OVERRIDE_ROOT/core" ]]
[[ $ZEKO_UI_ROOT == "$OVERRIDE_ROOT/web" ]]

ZEKO_ROOT=$TMP_DIR/missing
if zeko_resolve_companion_repo "$SETTLEMENT_ROOT" ZEKO_ROOT zeko src/app/zeko \
    2>"$TMP_DIR/missing.log"; then
  echo "Missing companion checkout unexpectedly resolved" >&2
  exit 1
fi
grep -q 'set ZEKO_ROOT or ZEKO_WORKSPACE_ROOT' "$TMP_DIR/missing.log"

CLEAN_REPO=$TMP_DIR/clean-repo
CLEAN_WORKTREE=$TMP_DIR/clean-worktree
git init -q "$CLEAN_REPO"
git -C "$CLEAN_REPO" config user.email test@example.invalid
git -C "$CLEAN_REPO" config user.name 'Workspace test'
touch "$CLEAN_REPO/tracked"
git -C "$CLEAN_REPO" add tracked
git -C "$CLEAN_REPO" commit -qm initial
git -C "$CLEAN_REPO" worktree add -q --detach "$CLEAN_WORKTREE"
zeko_is_clean_checkout "$CLEAN_REPO"
zeko_is_clean_checkout "$CLEAN_WORKTREE"
touch "$CLEAN_WORKTREE/untracked"
if zeko_is_clean_checkout "$CLEAN_WORKTREE"; then
  echo "Dirty linked worktree unexpectedly passed the clean-check" >&2
  exit 1
fi
if zeko_is_clean_checkout ''; then
  echo "Empty checkout path unexpectedly passed the clean-check" >&2
  exit 1
fi

echo "Workspace path resolution passed."
