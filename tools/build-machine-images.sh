#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 <settlement-vk-json> [testnet-dir]" >&2
  exit 2
}

[[ $# -ge 1 && $# -le 2 ]] || usage
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/tools/lib/workspace.sh"
zeko_resolve_companion_repo "$ROOT" ZEKO_ROOT zeko src/app/zeko
TESTNET_DIR=${2:-$ROOT/deploy/testnet}
NIX=${NIX:-$HOME/.nix-profile/bin/nix}
REGISTRY_ADDRESS=${REGISTRY_ADDRESS:-127.0.0.1:5000}
REGISTRY_CONTAINER=${REGISTRY_CONTAINER:-zeko-poc-registry}

VK_JSON=$(realpath "$1")
[[ $TESTNET_DIR == /* ]] || TESTNET_DIR="$ROOT/$TESTNET_DIR"
TESTNET_DIR=$(realpath "$TESTNET_DIR")
[[ $VK_JSON == "$ROOT/"* && -f $TESTNET_DIR/.env ]] || {
  echo "VK must be inside this repository and testnet identity must be initialized" >&2
  exit 1
}
[[ -x $NIX ]] || {
  echo "Missing Nix: $NIX" >&2
  exit 1
}
for command in docker git jq; do
  command -v "$command" >/dev/null || {
    echo "Missing command: $command" >&2
    exit 1
  }
done
for repo in "$ROOT" "$ZEKO_ROOT"; do
  [[ -d $repo/.git && -z $(git -C "$repo" status --porcelain) ]] || {
    echo "Source checkout must be clean before building images: $repo" >&2
    exit 1
  }
done

if ! docker inspect "$REGISTRY_CONTAINER" >/dev/null 2>&1; then
  docker pull registry:2.8.3
  docker run -d --restart unless-stopped \
    -p "127.0.0.1:${REGISTRY_ADDRESS##*:}:5000" \
    --name "$REGISTRY_CONTAINER" registry:2.8.3 >/dev/null
elif [[ $(docker inspect -f '{{.State.Running}}' "$REGISTRY_CONTAINER") != true ]]; then
  docker start "$REGISTRY_CONTAINER" >/dev/null
fi

settlement_commit=$(git -C "$ROOT" rev-parse HEAD)
zeko_commit=$(git -C "$ZEKO_ROOT" rev-parse HEAD)
gateway_tag="$REGISTRY_ADDRESS/zeko-gateway:${settlement_commit:0:12}"
zeko_tag="$REGISTRY_ADDRESS/zeko:${zeko_commit:0:12}"
zeko_da_tag="$REGISTRY_ADDRESS/zeko-da:${zeko_commit:0:12}"

docker build --pull --build-arg "SETTLEMENT_VK_JSON=${VK_JSON#"$ROOT/"}" \
  -t "$gateway_tag" "$ROOT"

load_nix_image() {
  local package=$1 tag=$2 loaded
  local image_path
  image_path=$("$NIX" build \
    "git+file://$ZEKO_ROOT?submodules=1#$package" \
    --accept-flake-config --no-link --print-out-paths)
  loaded=$(docker load <"$image_path")
  local source_image
  source_image=$(awk -F': ' '/Loaded image:/ { print $2 }' <<<"$loaded")
  [[ -n $source_image ]] || {
    echo "Could not determine loaded $package image name" >&2
    exit 1
  }
  docker tag "$source_image" "$tag"
}

load_nix_image zeko-image "$zeko_tag"
load_nix_image zeko-da-image "$zeko_da_tag"

for image in "$gateway_tag" "$zeko_tag" "$zeko_da_tag"; do
  docker push "$image"
done
docker pull postgres:16-alpine
docker pull rabbitmq:4.1-management

repo_digest() {
  local image=$1 prefix=$2
  docker image inspect "$image" --format '{{json .RepoDigests}}' \
    | jq -er --arg prefix "$prefix@sha256:" \
      '.[] | select(startswith($prefix))'
}

GATEWAY_IMAGE=$(repo_digest "$gateway_tag" "$REGISTRY_ADDRESS/zeko-gateway")
ZEKO_IMAGE=$(repo_digest "$zeko_tag" "$REGISTRY_ADDRESS/zeko")
ZEKO_DA_IMAGE=$(repo_digest "$zeko_da_tag" "$REGISTRY_ADDRESS/zeko-da")
POSTGRES_IMAGE=$(repo_digest postgres:16-alpine postgres)
RABBITMQ_IMAGE=$(repo_digest rabbitmq:4.1-management rabbitmq)

awk -v gateway="$GATEWAY_IMAGE" -v zeko="$ZEKO_IMAGE" \
  -v zeko_da="$ZEKO_DA_IMAGE" -v postgres="$POSTGRES_IMAGE" \
  -v rabbitmq="$RABBITMQ_IMAGE" '
    /^GATEWAY_IMAGE=/ { print "GATEWAY_IMAGE=\047" gateway "\047"; next }
    /^ZEKO_IMAGE=/ { print "ZEKO_IMAGE=\047" zeko "\047"; next }
    /^ZEKO_DA_IMAGE=/ { print "ZEKO_DA_IMAGE=\047" zeko_da "\047"; next }
    /^POSTGRES_IMAGE=/ { print "POSTGRES_IMAGE=\047" postgres "\047"; next }
    /^RABBITMQ_IMAGE=/ { print "RABBITMQ_IMAGE=\047" rabbitmq "\047"; next }
    { print }
  ' "$TESTNET_DIR/.env" >"$TESTNET_DIR/.env.tmp"
mv "$TESTNET_DIR/.env.tmp" "$TESTNET_DIR/.env"
chmod 0600 "$TESTNET_DIR/.env"

mkdir -p "$TESTNET_DIR/artifacts"
jq -n --arg gateway "$GATEWAY_IMAGE" --arg zeko "$ZEKO_IMAGE" \
  --arg zekoDa "$ZEKO_DA_IMAGE" --arg postgres "$POSTGRES_IMAGE" \
  --arg rabbitmq "$RABBITMQ_IMAGE" --arg vkSha256 "$(sha256sum "$VK_JSON" | awk '{print $1}')" \
  --arg settlementCommit "$settlement_commit" --arg zekoCommit "$zeko_commit" \
  '{schemaVersion:1,registry:"loopback",gateway:$gateway,zeko:$zeko,
    zekoDa:$zekoDa,postgres:$postgres,rabbitmq:$rabbitmq,
    settlementVkSha256:$vkSha256,
    sourceRevisions:{ethereumSettlement:$settlementCommit,zeko:$zekoCommit}}' \
  >"$TESTNET_DIR/artifacts/images.json"

jq -n --argjson images "$(<"$TESTNET_DIR/artifacts/images.json")" \
  '{status:"built",images:$images}'
