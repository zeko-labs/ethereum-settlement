#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/tools/lib/workspace.sh"
zeko_resolve_companion_repo "$ROOT" ZEKO_ROOT zeko src/app/zeko
OUTPUT_DIR=${1:-$ROOT/build/poc/bridge-fixtures}
ENV_FILE=${POC_ENV_FILE:-$ROOT/build/poc/deployment.env}
NIX=${NIX:-$HOME/.nix-profile/bin/nix}
CAST=${CAST:-$HOME/.foundry/bin/cast}

[[ -d "$ZEKO_ROOT" && -f "$ENV_FILE" && -x "$NIX" && -x "$CAST" ]] || {
  echo "Missing Zeko checkout, PoC deployment environment, Nix, or cast" >&2
  exit 1
}

mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR=$(realpath "$OUTPUT_DIR")
set -a
source "$ENV_FILE"
set +a
if [[ ${POC_REUSE_OCAML_EXPORT:-false} != true ]]; then
  rm -f "$OUTPUT_DIR"/settlement-*.json "$OUTPUT_DIR"/bridge-scenario.json \
    "$OUTPUT_DIR"/bridge-genesis-ledger.json
  rm -rf "$OUTPUT_DIR"/deposit-sync "$OUTPUT_DIR"/withdrawal

  export ZEKO_ETHEREUM_SETTLEMENT_FIXTURE_DIR="$OUTPUT_DIR"
  export ZEKO_ETHEREUM_SETTLEMENT_FIXTURE_ONLY=true
  export ZEKO_ETHEREUM_BRIDGE_EXPORT_ONLY=true
  unset ZEKO_ETHEREUM_SEQUENTIAL_EXPORT_ONLY

  echo "Running the real OCaml deposit/finalize/withdrawal scenario with DA quorum 2 of 3..."
  echo "Bridge settlement exports: $OUTPUT_DIR"
  (
    cd "$ZEKO_ROOT"
    "$NIX" develop "git+file://$ZEKO_ROOT?submodules=1" --accept-flake-config -c \
      src/app/zeko/sequencer/tests/run-sequencer-test.sh real 1 false true
  )
else
  echo "Revalidating existing OCaml bridge export: $OUTPUT_DIR"
fi

mapfile -t exports < <(find "$OUTPUT_DIR" -maxdepth 1 -type f \
  -name 'settlement-*.json' -print | sort)
if [[ ${#exports[@]} -ne 2 ]]; then
  echo "Expected exactly two bridge settlement exports, got ${#exports[@]}" >&2
  exit 1
fi
[[ -f "$OUTPUT_DIR/bridge-scenario.json" ]] || {
  echo "OCaml bridge scenario manifest was not exported" >&2
  exit 1
}
[[ -f "$OUTPUT_DIR/bridge-genesis-ledger.json" ]] || {
  echo "OCaml bridge genesis ledger was not exported" >&2
  exit 1
}
[[ $(jq '[.daPublicKeys[]] | unique | length' \
  "$OUTPUT_DIR/bridge-scenario.json") == 3 ]] || {
  echo "OCaml bridge scenario must bind three distinct DA public keys" >&2
  exit 1
}

derive_state_after() {
  jq -c '
    .proof.binding.stateBefore.fields as $before
    | .proof.binding.accountUpdateBody.fieldElements as $body
    | [$before[0],$body[3],$body[4],$body[5],$body[6],$before[5],$before[6],$body[9]]
  ' "$1"
}

first=${exports[0]}
second=${exports[1]}
if ! jq -e --argjson expected "$(derive_state_after "$first")" \
    '.proof.binding.stateBefore.fields == $expected' "$second" >/dev/null; then
  if jq -e --argjson expected "$(derive_state_after "$second")" \
      '.proof.binding.stateBefore.fields == $expected' "$first" >/dev/null; then
    first=${exports[1]}
    second=${exports[0]}
  else
    echo "The two OCaml commits do not form one proof-bound state chain" >&2
    exit 1
  fi
fi

expected_bridge=${BRIDGE_CONTRACT_ADDRESS,,}
for fixture in "$first" "$second"; do
  bridge=$(jq -r '.proof.innerActionBatch.bridgeAddress' "$fixture")
  if [[ ${bridge,,} != "$expected_bridge" ]]; then
    echo "Fixture bridge $bridge does not match $BRIDGE_CONTRACT_ADDRESS" >&2
    exit 1
  fi
done

[[ $(jq '.proof.innerActionBatch.actions | length' "$first") == 0 ]] || {
  echo "Deposit synchronization commit unexpectedly contains inner actions" >&2
  exit 1
}
[[ $(jq '.proof.innerActionBatch.actions | length' "$second") == 1 ]] || {
  echo "Withdrawal commit must contain exactly one inner action" >&2
  exit 1
}

sync_length=$(
  "$CAST" to-dec "$(jq -r '.proof.binding.actions[0][5]' "$first")"
)
[[ $sync_length == 1 ]] || {
  echo "Deposit commit synchronized $sync_length outer actions, expected 1" >&2
  exit 1
}

before_inner_length=$(
  "$CAST" to-dec "$(jq -r '.proof.binding.stateBefore.fields[4]' "$second")"
)
after_inner_length=$(
  "$CAST" to-dec "$(derive_state_after "$second" | jq -r '.[4]')"
)
[[ $after_inner_length == $((before_inner_length + 1)) ]] || {
  echo "Withdrawal commit did not advance the Pickles-bound inner action length by one" >&2
  exit 1
}

[[ $(jq -r '.proof.innerActionBatch.actions[0] | has("withdrawal")' \
  "$second") == true ]] || {
  echo "OCaml archive did not bind the native withdrawal preimage" >&2
  exit 1
}
expected_recipient=$(jq -r '.withdrawalRecipient | ascii_downcase' \
  "$OUTPUT_DIR/bridge-scenario.json")
actual_recipient=$(jq -r \
  '.proof.innerActionBatch.actions[0].withdrawal.recipient | ascii_downcase' \
  "$second")
expected_amount=$(jq -r '.withdrawalAmountZeko' \
  "$OUTPUT_DIR/bridge-scenario.json")
actual_amount=$(jq -r \
  '.proof.innerActionBatch.actions[0].withdrawal.amount' "$second")
[[ $actual_recipient == "$expected_recipient" && \
   $actual_amount == "$expected_amount" ]] || {
  echo "Exported withdrawal preimage does not match the OCaml scenario" >&2
  exit 1
}
[[ $(jq -r '.zekoRecipientIsOdd' "$OUTPUT_DIR/bridge-scenario.json") == false ]]
initial_action_state=$(jq -r '.outerActionStateBeforeDeposit | ascii_downcase' \
  "$OUTPUT_DIR/bridge-scenario.json")
deposit_action_state=$(jq -r '.outerActionStateAfterDeposit | ascii_downcase' \
  "$OUTPUT_DIR/bridge-scenario.json")
commit_action_state=$(jq -r \
  '.proof.binding.accountUpdateBody.fieldElements[36] | ascii_downcase' "$first")
[[ $initial_action_state != "$deposit_action_state" && \
   $deposit_action_state == "$commit_action_state" ]] || {
  echo "OCaml deposit action-state checkpoints do not match the accepting commit" >&2
  exit 1
}

reference_vk_sha=
for entry in "deposit-sync:$first" "withdrawal:$second"; do
  name=${entry%%:*}
  fixture=${entry#*:}
  directory="$OUTPUT_DIR/$name"
  mkdir -p "$directory"
  cp "$fixture" "$directory/settlement.json"
  jq -jr '.proof.vkJson' "$fixture" >"$directory/vk.serde.json"
  jq -jr '.proof.proofJson' "$fixture" >"$directory/proof.serde.json"
  jq -jr '.proof.publicInputSkeletonJson' "$fixture" \
    >"$directory/public_input_skeleton.json"
  jq -jr '.proof.appStatementJson' "$fixture" \
    >"$directory/app_statement.json"
  vk_sha=$(sha256sum "$directory/vk.serde.json" | awk '{print $1}')
  if [[ -z $reference_vk_sha ]]; then
    reference_vk_sha=$vk_sha
  elif [[ $vk_sha != "$reference_vk_sha" ]]; then
    echo "Bridge fixture verification keys do not match" >&2
    exit 1
  fi
done

jq -n --arg directory "$OUTPUT_DIR" --arg bridge "$BRIDGE_CONTRACT_ADDRESS" \
  --arg vkSha256 "$reference_vk_sha" --argjson settlements 2 \
  '{directory:$directory,settlements:$settlements,daQuorum:"2-of-3",
    bridgeAddress:$bridge,vkSha256:$vkSha256,
    depositSynchronized:true,withdrawalPreimageBound:true}'
echo "No SP1 proof was requested or generated."
