#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/tools/lib/workspace.sh"
zeko_resolve_companion_repo "$ROOT" ZEKO_ROOT zeko src/app/zeko
OUTPUT_DIR=${1:-$ROOT/build/poc/bridge-fixtures}
ENV_FILE=${POC_ENV_FILE:-$ROOT/build/poc/deployment.env}
NIX=${NIX:-$HOME/.nix-profile/bin/nix}
CAST=${CAST:-$HOME/.foundry/bin/cast}
BRIDGE_ASSET=${BRIDGE_ASSET:-native}

case "$BRIDGE_ASSET" in
native | erc20) ;;
*)
  echo "Unsupported BRIDGE_ASSET: $BRIDGE_ASSET" >&2
  exit 1
  ;;
esac

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
  rm -rf "$OUTPUT_DIR"/registration "$OUTPUT_DIR"/deposit-sync \
    "$OUTPUT_DIR"/withdrawal

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
expected_exports=2
if [[ $BRIDGE_ASSET == erc20 ]]; then
  expected_exports=3
fi
if [[ ${#exports[@]} -ne $expected_exports ]]; then
  echo "Expected $expected_exports bridge settlement exports, got ${#exports[@]}" >&2
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

fixtures=()
for candidate in "${exports[@]}"; do
  chain=("$candidate")
  while [[ ${#chain[@]} -lt $expected_exports ]]; do
    expected=$(derive_state_after "${chain[-1]}")
    next=
    for possible in "${exports[@]}"; do
      used=false
      for selected in "${chain[@]}"; do
        [[ $selected == "$possible" ]] && used=true
      done
      if [[ $used == false ]] && jq -e --argjson expected "$expected" \
          '.proof.binding.stateBefore.fields == $expected' "$possible" >/dev/null; then
        next=$possible
        break
      fi
    done
    [[ -n $next ]] || break
    chain+=("$next")
  done
  if [[ ${#chain[@]} -eq $expected_exports ]]; then
    fixtures=("${chain[@]}")
    break
  fi
done
if [[ ${#fixtures[@]} -ne $expected_exports ]]; then
  echo "The OCaml commits do not form one proof-bound state chain" >&2
  exit 1
fi

expected_bridge=${BRIDGE_CONTRACT_ADDRESS,,}
for fixture in "${fixtures[@]}"; do
  bridge=$(jq -r '.proof.innerActionBatch.bridgeAddress' "$fixture")
  if [[ ${bridge,,} != "$expected_bridge" ]]; then
    echo "Fixture bridge $bridge does not match $BRIDGE_CONTRACT_ADDRESS" >&2
    exit 1
  fi
done

if [[ $BRIDGE_ASSET == erc20 ]]; then
  registration=${fixtures[0]}
  deposit_sync=${fixtures[1]}
  withdrawal=${fixtures[2]}
  [[ $(jq '.proof.assetRegistryBatch.appends | length' "$registration") == 2 ]] || {
    echo "Registration commit must bind exactly two registry appends" >&2
    exit 1
  }
  [[ $(jq '.proof.innerActionBatch.actions | length' "$registration") == 0 ]] || {
    echo "Registration commit unexpectedly contains inner actions" >&2
    exit 1
  }
  [[ $(jq '.proof.assetRegistryBatch == null' "$deposit_sync") == true ]] || {
    echo "Deposit synchronization commit unexpectedly contains registry appends" >&2
    exit 1
  }
  expected_withdrawals=2
else
  registration=
  deposit_sync=${fixtures[0]}
  withdrawal=${fixtures[1]}
  expected_withdrawals=1
fi

[[ $(jq '.proof.innerActionBatch.actions | length' "$deposit_sync") == 0 ]] || {
  echo "Deposit synchronization commit unexpectedly contains inner actions" >&2
  exit 1
}
[[ $(jq '.proof.innerActionBatch.actions | length' "$withdrawal") == \
  "$expected_withdrawals" ]] || {
  echo "Withdrawal commit must contain exactly $expected_withdrawals inner actions" >&2
  exit 1
}

sync_length=$(
  "$CAST" to-dec "$(jq -r '.proof.binding.actions[0][5]' "$deposit_sync")"
)
expected_deposits=$expected_withdrawals
[[ $sync_length == "$expected_deposits" ]] || {
  echo "Deposit commit synchronized $sync_length outer actions, expected $expected_deposits" >&2
  exit 1
}

before_inner_length=$(
  "$CAST" to-dec "$(jq -r '.proof.binding.stateBefore.fields[4]' "$withdrawal")"
)
after_inner_length=$(
  "$CAST" to-dec "$(derive_state_after "$withdrawal" | jq -r '.[4]')"
)
[[ $after_inner_length == $((before_inner_length + expected_withdrawals)) ]] || {
  echo "Withdrawal commit did not advance the Pickles-bound inner action length by $expected_withdrawals" >&2
  exit 1
}

[[ $(jq -r '.bridgeAsset // "native"' "$OUTPUT_DIR/bridge-scenario.json") == \
  "$BRIDGE_ASSET" ]] || {
  echo "OCaml bridge scenario asset does not match BRIDGE_ASSET" >&2
  exit 1
}
expected_recipient=$(jq -r '.withdrawalRecipient | ascii_downcase' \
  "$OUTPUT_DIR/bridge-scenario.json")
expected_amount=$(jq -r '.withdrawalAmountZeko' \
  "$OUTPUT_DIR/bridge-scenario.json")
if [[ $BRIDGE_ASSET == native ]]; then
  [[ $(jq -r '.proof.innerActionBatch.actions[0] | has("withdrawal")' \
    "$withdrawal") == true ]] || {
    echo "OCaml archive did not bind the native withdrawal preimage" >&2
    exit 1
  }
  actual_recipient=$(jq -r \
    '.proof.innerActionBatch.actions[0].withdrawal.recipient | ascii_downcase' \
    "$withdrawal")
  actual_amount=$(jq -r \
    '.proof.innerActionBatch.actions[0].withdrawal.amount' "$withdrawal")
  [[ $actual_recipient == "$expected_recipient" && \
     $actual_amount == "$expected_amount" ]] || {
    echo "Exported native withdrawal preimage does not match the OCaml scenario" >&2
    exit 1
  }
else
  for index in 0 1; do
    [[ $(jq -r --argjson index "$index" \
      '.proof.innerActionBatch.actions[$index] | has("tokenWithdrawal")' \
      "$withdrawal") == true ]] || {
      echo "OCaml archive did not bind ERC20 withdrawal preimage $index" >&2
      exit 1
    }
    expected_token=$(jq -r --argjson index "$index" \
      '.ethereumAssets[$index].record.ethereumToken | ascii_downcase' \
      "$OUTPUT_DIR/bridge-scenario.json")
    expected_asset=$(jq -r --argjson index "$index" \
      '.ethereumAssets[$index].record.assetId | ascii_downcase' \
      "$OUTPUT_DIR/bridge-scenario.json")
    actual_token=$(jq -r --argjson index "$index" \
      '.proof.innerActionBatch.actions[$index].tokenWithdrawal.token | ascii_downcase' \
      "$withdrawal")
    actual_asset=$(jq -r --argjson index "$index" \
      '.proof.innerActionBatch.actions[$index].tokenWithdrawal.assetId | ascii_downcase' \
      "$withdrawal")
    actual_recipient=$(jq -r --argjson index "$index" \
      '.proof.innerActionBatch.actions[$index].tokenWithdrawal.recipient | ascii_downcase' \
      "$withdrawal")
    actual_amount=$(jq -r --argjson index "$index" \
      '.proof.innerActionBatch.actions[$index].tokenWithdrawal.amount' \
      "$withdrawal")
    params_length=$(jq --argjson index "$index" \
      '.proof.innerActionBatch.actions[$index].tokenWithdrawal.paramsFields | length' \
      "$withdrawal")
    [[ $actual_token == "$expected_token" && \
       $actual_asset == "$expected_asset" && \
       $actual_recipient == "$expected_recipient" && \
       $actual_amount == "$expected_amount" && $params_length -gt 0 ]] || {
      echo "Exported ERC20 withdrawal $index does not match the OCaml scenario" >&2
      exit 1
    }
  done
fi
[[ $(jq -r '.zekoRecipientIsOdd' "$OUTPUT_DIR/bridge-scenario.json") == false ]]
initial_action_state=$(jq -r '.outerActionStateBeforeDeposit | ascii_downcase' \
  "$OUTPUT_DIR/bridge-scenario.json")
initial_registration_action_state=$(jq -r \
  '.outerActionStateBeforeRegistration | ascii_downcase' \
  "$OUTPUT_DIR/bridge-scenario.json")
deposit_action_state=$(jq -r '.outerActionStateAfterDeposit | ascii_downcase' \
  "$OUTPUT_DIR/bridge-scenario.json")
commit_action_state=$(jq -r \
  '.proof.binding.accountUpdateBody.fieldElements[36] | ascii_downcase' \
  "$deposit_sync")
[[ $initial_action_state != "$deposit_action_state" && \
   $deposit_action_state == "$commit_action_state" ]] || {
  echo "OCaml deposit action-state checkpoints do not match the accepting commit" >&2
  exit 1
}
if [[ $BRIDGE_ASSET == erc20 ]]; then
  registration_action_state=$(jq -r \
    '.proof.binding.accountUpdateBody.fieldElements[36] | ascii_downcase' \
    "$registration")
  [[ $registration_action_state == "$initial_registration_action_state" ]] || {
    echo "Registry-only commit unexpectedly changed the outer action state" >&2
    exit 1
  }
fi

reference_vk_sha=
entries=("deposit-sync:$deposit_sync" "withdrawal:$withdrawal")
if [[ $BRIDGE_ASSET == erc20 ]]; then
  entries=("registration:$registration" "${entries[@]}")
fi
for entry in "${entries[@]}"; do
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
  --arg vkSha256 "$reference_vk_sha" --argjson settlements "$expected_exports" \
  --argjson registrationSettlements "$([[ $BRIDGE_ASSET == erc20 ]] && echo 1 || echo 0)" \
  --argjson bridgeSettlements 2 \
  '{directory:$directory,settlements:$settlements,daQuorum:"2-of-3",
    bridgeAddress:$bridge,vkSha256:$vkSha256,
    registrationSettlements:$registrationSettlements,
    bridgeSettlements:$bridgeSettlements,
    depositSynchronized:true,withdrawalPreimageBound:true}'
echo "No SP1 proof was requested or generated."
