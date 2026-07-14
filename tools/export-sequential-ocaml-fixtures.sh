#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ZEKO_ROOT=${ZEKO_ROOT:-/root/zeko}
OUTPUT_DIR=${1:-$ROOT/build/poc/sequential-fixtures}
ENV_FILE=${POC_ENV_FILE:-$ROOT/build/poc/deployment.env}
NIX=${NIX:-$HOME/.nix-profile/bin/nix}

[[ -d "$ZEKO_ROOT" && -f "$ENV_FILE" && -x "$NIX" ]] || {
  echo "Missing Zeko checkout, PoC deployment environment, or Nix" >&2
  exit 1
}
mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR=$(realpath "$OUTPUT_DIR")
rm -f "$OUTPUT_DIR"/settlement-*.json
rm -rf "$OUTPUT_DIR"/sequence-*

set -a
source "$ENV_FILE"
set +a
export ZEKO_ETHEREUM_SETTLEMENT_FIXTURE_DIR="$OUTPUT_DIR"
export ZEKO_ETHEREUM_SETTLEMENT_FIXTURE_ONLY=true
export ZEKO_ETHEREUM_SEQUENTIAL_EXPORT_ONLY=true

echo "Running the real OCaml sequencer test with three DA nodes and quorum 2..."
echo "Sequential settlement exports: $OUTPUT_DIR"
(
  cd "$ZEKO_ROOT"
  "$NIX" develop "git+file://$ZEKO_ROOT?submodules=1" --accept-flake-config -c \
    src/app/zeko/sequencer/tests/run-sequencer-test.sh real 1 false true
)

mapfile -t exports < <(find "$OUTPUT_DIR" -maxdepth 1 -type f \
  -name 'settlement-*.json' -print | sort)
if [[ ${#exports[@]} -lt 3 ]]; then
  echo "Expected at least three settlement exports, got ${#exports[@]}" >&2
  exit 1
fi

derive_state_after() {
  jq -c '
    .proof.binding.stateBefore.fields as $before
    | .proof.binding.accountUpdateBody.fieldElements as $body
    | [$before[0],$body[3],$body[4],$body[5],$body[6],$before[5],$before[6],$body[9]]
  ' "$1"
}

# sequencer_test.exe contains multiple independent scenarios. Find an actual
# three-commit state chain instead of assuming every exported command belongs
# to the same outer account or that fee-payer nonces are globally ordered.
fixtures=()
for ((first = 0; first < ${#exports[@]}; first++)); do
  first_after=$(derive_state_after "${exports[first]}")
  for ((second = 0; second < ${#exports[@]}; second++)); do
    ((second != first)) || continue
    second_before=$(jq -c '.proof.binding.stateBefore.fields' "${exports[second]}")
    [[ $first_after == "$second_before" ]] || continue
    second_after=$(derive_state_after "${exports[second]}")
    for ((third = 0; third < ${#exports[@]}; third++)); do
      ((third != first && third != second)) || continue
      third_before=$(jq -c '.proof.binding.stateBefore.fields' "${exports[third]}")
      if [[ $second_after == "$third_before" ]]; then
        fixtures=("${exports[first]}" "${exports[second]}" "${exports[third]}")
        break 3
      fi
    done
  done
done
if [[ ${#fixtures[@]} -ne 3 ]]; then
  echo "Could not find three proof-bound consecutive commits in ${#exports[@]} exports" >&2
  exit 1
fi

for ((index = 0; index < ${#fixtures[@]}; index++)); do
  sequence_dir=$(printf '%s/sequence-%04d' "$OUTPUT_DIR" "$index")
  mkdir -p "$sequence_dir"
  cp "${fixtures[index]}" "$sequence_dir/settlement.json"
  jq -jr '.proof.vkJson' "${fixtures[index]}" >"$sequence_dir/vk.serde.json"
  jq -jr '.proof.proofJson' "${fixtures[index]}" >"$sequence_dir/proof.serde.json"
  jq -jr '.proof.publicInputSkeletonJson' "${fixtures[index]}" \
    >"$sequence_dir/public_input_skeleton.json"
  jq -jr '.proof.appStatementJson' "${fixtures[index]}" \
    >"$sequence_dir/app_statement.json"
done

expected_bridge=${BRIDGE_CONTRACT_ADDRESS,,}
reference_vk_sha=
for sequence_dir in "$OUTPUT_DIR"/sequence-*; do
  bridge=$(jq -r '.proof.innerActionBatch.bridgeAddress' \
    "$sequence_dir/settlement.json")
  if [[ ${bridge,,} != "$expected_bridge" ]]; then
    echo "Fixture bridge $bridge does not match $BRIDGE_CONTRACT_ADDRESS" >&2
    exit 1
  fi
  vk_sha=$(sha256sum "$sequence_dir/vk.serde.json" | awk '{print $1}')
  if [[ -z $reference_vk_sha ]]; then
    reference_vk_sha=$vk_sha
  elif [[ $vk_sha != "$reference_vk_sha" ]]; then
    echo "Sequential fixture verification keys do not match" >&2
    exit 1
  fi
done

jq -n --arg directory "$OUTPUT_DIR" --argjson count "${#fixtures[@]}" \
  --argjson totalExports "${#exports[@]}" --arg bridge "$BRIDGE_CONTRACT_ADDRESS" \
  --arg vkSha256 "$reference_vk_sha" \
  '{directory:$directory,count:$count,totalExports:$totalExports,
    sequentialOuterStates:true,bridgeAddress:$bridge,vkSha256:$vkSha256}'
echo "No SP1 proof was requested or generated."
