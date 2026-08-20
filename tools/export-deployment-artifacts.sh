#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 [output-directory]" >&2
  exit 2
}

[[ $# -le 1 ]] || usage
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/tools/lib/workspace.sh"
zeko_resolve_companion_repo "$ROOT" ZEKO_ROOT zeko src/app/zeko

OUTPUT_DIRECTORY=$(realpath -m \
  "${1:-$ROOT/build/poc/deployment-fixture}")
ENV_FILE=${ZEKO_DEPLOYMENT_ENV_FILE:-}
ENV_FILE=${ENV_FILE:-${POC_ENV_FILE:-$ROOT/deploy/testnet/secrets/fixture-keys.env}}
NIX=${NIX:-$HOME/.nix-profile/bin/nix}

[[ -d $ZEKO_ROOT && -f $ENV_FILE && -x $NIX ]] || {
  echo "Missing Zeko checkout, fixture environment, or Nix" >&2
  exit 1
}
[[ ! -e $OUTPUT_DIRECTORY ]] || {
  echo "Refusing to overwrite fixture: $OUTPUT_DIRECTORY" >&2
  exit 1
}
for command in jq sha256sum; do
  command -v "$command" >/dev/null || {
    echo "Missing command: $command" >&2
    exit 1
  }
done

set -a
source "$ENV_FILE"
set +a
for variable in ZEKO_CIRCUITS_CONFIG ZEKO_DEPLOY_CONFIG; do
  [[ -f ${!variable:-} ]] || {
    echo "$variable must name an existing file" >&2
    exit 1
  }
done
zeko_is_clean_checkout "$ZEKO_ROOT" || {
  echo "Zeko checkout must be clean before exporting a fixture" >&2
  exit 1
}

DA_NODE_COUNT=${DA_NODE_COUNT:-3}
DA_QUORUM=${DA_QUORUM:-2}
[[ $DA_NODE_COUNT =~ ^[1-3]$ ]] || {
  echo "DA_NODE_COUNT must be between 1 and 3" >&2
  exit 1
}
[[ $DA_QUORUM =~ ^[1-3]$ && $DA_QUORUM -le $DA_NODE_COUNT ]] || {
  echo "DA_QUORUM must be between 1 and DA_NODE_COUNT" >&2
  exit 1
}

if [[ -z ${ZEKO_DEPLOYMENT_SEQUENCER_PRIVATE_KEY:-} ]]; then
  ZEKO_DEPLOYMENT_SEQUENCER_PRIVATE_KEY=${ZEKO_TEST_SEQUENCER_SIGNER_PRIVATE_KEY:-}
fi
[[ -n $ZEKO_DEPLOYMENT_SEQUENCER_PRIVATE_KEY ]] || {
  echo "Fixture environment is missing the sequencer private key" >&2
  exit 1
}
export ZEKO_DEPLOYMENT_SEQUENCER_PRIVATE_KEY
for ((index = 1; index <= DA_NODE_COUNT; index++)); do
  variable="ZEKO_DEPLOYMENT_DA${index}_PRIVATE_KEY"
  legacy_variable="DA${index}_SIGNER_PRIVATE_KEY"
  value=${!variable:-}
  [[ -n $value ]] || value=${!legacy_variable:-}
  [[ -n $value ]] || {
    echo "Fixture environment is missing DA private key $index" >&2
    exit 1
  }
  printf -v "$variable" '%s' "$value"
  export "$variable"
done

zeko_revision=$(git -C "$ZEKO_ROOT" rev-parse HEAD)
circuits_sha256=$(sha256sum "$ZEKO_CIRCUITS_CONFIG" | awk '{print $1}')
mkdir -p "$(dirname "$OUTPUT_DIRECTORY")"
STAGING_DIRECTORY=$(mktemp -d \
  "$(dirname "$OUTPUT_DIRECTORY")/.deployment-fixture.XXXXXX")
cleanup() {
  rm -rf "$STAGING_DIRECTORY"
}
trap cleanup EXIT INT TERM

(
  cd "$ZEKO_ROOT"
  "$NIX" develop "git+file://$ZEKO_ROOT?submodules=1" \
    --accept-flake-config -c \
    src/app/zeko/sequencer/run-deployment-export.sh \
    "$STAGING_DIRECTORY" "$DA_NODE_COUNT" "$DA_QUORUM"
)

mapfile -t settlements < <(find "$STAGING_DIRECTORY" -maxdepth 1 \
  -type f -name 'settlement-*.json' -print)
[[ ${#settlements[@]} -eq 1 ]] || {
  echo "Expected one settlement, got ${#settlements[@]}" >&2
  exit 1
}
for file in deployment-manifest.json genesis-ledger.json; do
  [[ -f "$STAGING_DIRECTORY/$file" ]] || {
    echo "Zeko exporter did not produce $file" >&2
    exit 1
  }
done
mv "${settlements[0]}" "$STAGING_DIRECTORY/settlement.json"

manifest="$STAGING_DIRECTORY/deployment-manifest.json"
settlement="$STAGING_DIRECTORY/settlement.json"
jq -e --argjson nodeCount "$DA_NODE_COUNT" --argjson quorum "$DA_QUORUM" \
  --slurpfile settlement "$settlement" '
    .schemaVersion == 1
    and .daNodeCount == $nodeCount
    and .daQuorum == $quorum
    and (.daPublicKeys | length) == $nodeCount
    and (.daPublicKeys | unique | length) == $nodeCount
    and .outerPublicKey == $settlement[0].outerAccountPublicKey
    and ((.outerActionState | ascii_downcase) ==
      ($settlement[0].proof.binding.accountUpdateBody.fieldElements[36] |
        ascii_downcase))
    and ((.genesisLedgerField | ascii_downcase) ==
      ($settlement[0].proof.binding.stateBefore.fields[2] | ascii_downcase))
    and ((.daCommitment | ascii_downcase) ==
      ($settlement[0].proof.binding.stateBefore.fields[6] | ascii_downcase))
  ' "$manifest" >/dev/null || {
  echo "Zeko fixture manifest does not match its settlement" >&2
  exit 1
}

jq --arg zekoSourceRevision "$zeko_revision" \
  --arg circuitsConfigSha256 "$circuits_sha256" \
  '. + {zekoSourceRevision:$zekoSourceRevision,
    circuitsConfigSha256:$circuitsConfigSha256}' "$manifest" \
  >"$STAGING_DIRECTORY/deployment-manifest.tmp.json"
mv "$STAGING_DIRECTORY/deployment-manifest.tmp.json" "$manifest"

jq -jr '.proof.vkJson' "$settlement" >"$STAGING_DIRECTORY/vk.serde.json"
jq -jr '.proof.proofJson' "$settlement" \
  >"$STAGING_DIRECTORY/proof.serde.json"
jq -jr '.proof.publicInputSkeletonJson' "$settlement" \
  >"$STAGING_DIRECTORY/public_input_skeleton.json"
jq -jr '.proof.appStatementJson' "$settlement" \
  >"$STAGING_DIRECTORY/app_statement.json"

vk_sha256=$(sha256sum "$STAGING_DIRECTORY/vk.serde.json" | awk '{print $1}')
jq --arg settlementVkSha256 "$vk_sha256" \
  '. + {settlementVkSha256:$settlementVkSha256}' "$manifest" \
  >"$STAGING_DIRECTORY/deployment-manifest.tmp.json"
mv "$STAGING_DIRECTORY/deployment-manifest.tmp.json" "$manifest"
mv "$STAGING_DIRECTORY" "$OUTPUT_DIRECTORY"
trap - EXIT INT TERM

jq -n --arg directory "$OUTPUT_DIRECTORY" --arg vkSha256 "$vk_sha256" \
  --argjson daNodeCount "$DA_NODE_COUNT" --argjson daQuorum "$DA_QUORUM" \
  '{directory:$directory,vkSha256:$vkSha256,daNodeCount:$daNodeCount,
    daQuorum:$daQuorum,sp1ProofGenerated:false}'
