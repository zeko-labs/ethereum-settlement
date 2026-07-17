#!/usr/bin/env bash

zeko_resolve_companion_repo() {
  if [[ $# -ne 4 ]]; then
    echo "Usage: zeko_resolve_companion_repo <settlement-root> <variable> <repo-name> <marker>" >&2
    return 2
  fi

  local settlement_root=$1
  local variable=$2
  local repo_name=$3
  local marker=$4
  local workspace_root=${ZEKO_WORKSPACE_ROOT:-$(dirname "$settlement_root")}
  local repo_path=${!variable:-}

  [[ $workspace_root == /* ]] || workspace_root="$(dirname "$settlement_root")/$workspace_root"
  workspace_root=$(realpath -m "$workspace_root")
  repo_path=${repo_path:-$workspace_root/$repo_name}
  [[ $repo_path == /* ]] || repo_path="$workspace_root/$repo_path"
  repo_path=$(realpath -m "$repo_path")

  if [[ ! -e $repo_path/.git || ! -e $repo_path/$marker ]]; then
    echo "Missing $repo_name checkout at $repo_path" >&2
    echo "Place it next to ethereum-settlement, or set $variable or ZEKO_WORKSPACE_ROOT." >&2
    return 1
  fi

  printf -v "$variable" '%s' "$repo_path"
}
