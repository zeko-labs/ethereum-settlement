#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 <rpc-url> <admin-address> [fixture-dir] [output-dir]" >&2
  exit 2
}

[[ $# -ge 2 && $# -le 4 ]] || usage

RPC_URL=$1
ADMIN_ADDRESS=$2
FIXTURE_DIR=${3:-fixtures/zeko-local-e2e}
OUTPUT_DIR=${4:-build/poc}
FORGE=${FORGE:-$HOME/.foundry/bin/forge}

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
[[ $FIXTURE_DIR == /* ]] || FIXTURE_DIR="$ROOT/$FIXTURE_DIR"
[[ $OUTPUT_DIR == /* ]] || OUTPUT_DIR="$ROOT/$OUTPUT_DIR"
FIXTURE_DIR=$(realpath "$FIXTURE_DIR")
mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR=$(realpath "$OUTPUT_DIR")

[[ -x "$FORGE" ]] || {
  echo "Missing forge binary: $FORGE" >&2
  exit 1
}

[[ -f "$FIXTURE_DIR/vk.serde.json" ]] || {
  echo "Missing fixture VK: $FIXTURE_DIR/vk.serde.json" >&2
  exit 1
}
[[ -f "$FIXTURE_DIR/settlement.json" ]] || {
  echo "Missing fixture: $FIXTURE_DIR/settlement.json" >&2
  exit 1
}
BRIDGE_SCENARIO="$FIXTURE_DIR/../bridge-scenario.json"

vkey() {
  local program=$1
  "$ROOT/target/release/vkey" --program "$program" \
    | awk '/^0x[0-9a-fA-F]{64}$/ { value=$0 } END { if (value == "") exit 1; print value }'
}

echo "Computing SP1 program verification keys (no proving)..."
SETTLEMENT_VK_JSON="$FIXTURE_DIR/vk.serde.json" \
  cargo build --quiet --release --bin vkey
echo "Building the gateway with the selected settlement verification key..."
SETTLEMENT_VK_JSON="$FIXTURE_DIR/vk.serde.json" \
  cargo build --quiet --release -p zeko-proof-api
SETTLEMENT_PROGRAM_VKEY=$(vkey settlement)
BRIDGE_PROGRAM_VKEY=$(vkey bridge)
WITHDRAW_PROGRAM_VKEY=$(vkey withdraw)
SETTLEMENT_VK_HASH=0x$(sha256sum "$FIXTURE_DIR/vk.serde.json" | awk '{print $1}')
FIXTURE_SLOT_LOWER_HEX=$(jq -er '.proof.binding.actions[0][6]' \
  "$FIXTURE_DIR/settlement.json")
FIXTURE_SLOT_UPPER_HEX=$(jq -er '.proof.binding.actions[0][7]' \
  "$FIXTURE_DIR/settlement.json")
FORK_SLOT=$((FIXTURE_SLOT_LOWER_HEX))
FIXTURE_SLOT_UPPER=$((FIXTURE_SLOT_UPPER_HEX))
if ((FORK_SLOT > FIXTURE_SLOT_UPPER)); then
  echo "Invalid fixture slot range [$FORK_SLOT, $FIXTURE_SLOT_UPPER]" >&2
  exit 1
fi

prediction=$(
  cd "$ROOT/contracts"
  ADMIN_ADDRESS="$ADMIN_ADDRESS" "$FORGE" script \
    script/PredictPocDeployment.s.sol:PredictPocDeployment \
    --rpc-url "$RPC_URL" 2>&1
)

read_prediction() {
  local name=$1
  awk -v name="$name" '$0 ~ name { value=$NF } END { if (value == "") exit 1; print value }' \
    <<<"$prediction"
}

POC_FACTORY_ADDRESS=$(read_prediction POC_FACTORY_ADDRESS)
SETTLEMENT_IMPLEMENTATION_ADDRESS=$(read_prediction SETTLEMENT_IMPLEMENTATION_ADDRESS)
BRIDGE_IMPLEMENTATION_ADDRESS=$(read_prediction BRIDGE_IMPLEMENTATION_ADDRESS)
LOCAL_SP1_VERIFIER_ADDRESS=$(read_prediction LOCAL_SP1_VERIFIER_ADDRESS)
SETTLEMENT_CONTRACT_ADDRESS=$(read_prediction SETTLEMENT_CONTRACT_ADDRESS)
BRIDGE_CONTRACT_ADDRESS=$(read_prediction BRIDGE_CONTRACT_ADDRESS)

POC_MANIFEST_PATH="$OUTPUT_DIR/manifest.json"
export RPC_URL ADMIN_ADDRESS POC_FACTORY_ADDRESS
export SETTLEMENT_IMPLEMENTATION_ADDRESS BRIDGE_IMPLEMENTATION_ADDRESS
export SETTLEMENT_CONTRACT_ADDRESS BRIDGE_CONTRACT_ADDRESS
export SETTLEMENT_PROGRAM_VKEY BRIDGE_PROGRAM_VKEY WITHDRAW_PROGRAM_VKEY
export SETTLEMENT_VK_HASH POC_MANIFEST_PATH

(
  cd "$ROOT/contracts"
  "$FORGE" script script/WritePocManifest.s.sol:WritePocManifest \
    --rpc-url "$RPC_URL" >/dev/null
)

{
  echo "RPC_URL=$RPC_URL"
  echo "ADMIN_ADDRESS=$ADMIN_ADDRESS"
  echo "POC_FACTORY_ADDRESS=$POC_FACTORY_ADDRESS"
  echo "SETTLEMENT_IMPLEMENTATION_ADDRESS=$SETTLEMENT_IMPLEMENTATION_ADDRESS"
  echo "BRIDGE_IMPLEMENTATION_ADDRESS=$BRIDGE_IMPLEMENTATION_ADDRESS"
  echo "LOCAL_SP1_VERIFIER_ADDRESS=$LOCAL_SP1_VERIFIER_ADDRESS"
  echo "SETTLEMENT_CONTRACT_ADDRESS=$SETTLEMENT_CONTRACT_ADDRESS"
  echo "BRIDGE_CONTRACT_ADDRESS=$BRIDGE_CONTRACT_ADDRESS"
  echo "SETTLEMENT_PROGRAM_VKEY=$SETTLEMENT_PROGRAM_VKEY"
  echo "BRIDGE_PROGRAM_VKEY=$BRIDGE_PROGRAM_VKEY"
  echo "WITHDRAW_PROGRAM_VKEY=$WITHDRAW_PROGRAM_VKEY"
  echo "SETTLEMENT_VK_HASH=$SETTLEMENT_VK_HASH"
  echo "FORK_SLOT=$FORK_SLOT"
  echo "FIXTURE_SLOT_LOWER=$FORK_SLOT"
  echo "FIXTURE_SLOT_UPPER=$FIXTURE_SLOT_UPPER"
  echo "ZEKO_CIRCUITS_CONFIG=test"
  echo "ZEKO_ETHEREUM_BRIDGE_ADDRESS=$BRIDGE_CONTRACT_ADDRESS"
  echo "POC_MANIFEST_PATH=$POC_MANIFEST_PATH"
  for index in $(seq 0 7); do
    value=$(jq -r --argjson index "$index" \
      '.proof.binding.stateBefore.fields[$index]' \
      "$FIXTURE_DIR/settlement.json")
    echo "INITIAL_OUTER_STATE_$index=$value"
  done
  if [[ -f "$BRIDGE_SCENARIO" ]]; then
    echo "INITIAL_OUTER_ACTION_STATE=$(jq -er \
      '.outerActionStateBeforeDeposit' "$BRIDGE_SCENARIO")"
  else
    echo "INITIAL_OUTER_ACTION_STATE=$(jq -r \
      '.proof.binding.accountUpdateBody.fieldElements[36]' \
      "$FIXTURE_DIR/settlement.json")"
  fi
} >"$OUTPUT_DIR/deployment.env"

echo "Prepared PoC artifacts:"
echo "  $POC_MANIFEST_PATH"
echo "  $OUTPUT_DIR/deployment.env"
echo "No SP1 proof was requested or generated."
