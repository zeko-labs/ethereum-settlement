#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 <bridge-fixture-root> <circuits.json> <poc-output-dir> [testnet-dir]" >&2
  exit 2
}

[[ $# -ge 3 && $# -le 4 ]] || usage
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
FIXTURE_ROOT=$(realpath "$1")
CIRCUITS_CONFIG=$(realpath "$2")
POC_OUTPUT=$(realpath "$3")
TESTNET_DIR=${4:-$ROOT/deploy/testnet}
CAST=${CAST:-$HOME/.foundry/bin/cast}

[[ $TESTNET_DIR == /* ]] || TESTNET_DIR="$ROOT/$TESTNET_DIR"
mkdir -p "$TESTNET_DIR/config" "$TESTNET_DIR/artifacts"
TESTNET_DIR=$(realpath "$TESTNET_DIR")
for file in "$FIXTURE_ROOT/bridge-scenario.json" \
  "$FIXTURE_ROOT/bridge-genesis-ledger.json" \
  "$FIXTURE_ROOT/deposit-sync/settlement.json" \
  "$POC_OUTPUT/deployment.env" "$POC_OUTPUT/manifest.json" \
  "$CIRCUITS_CONFIG"; do
  [[ -f "$file" ]] || {
    echo "Missing input: $file" >&2
    exit 1
  }
done
[[ -x "$CAST" ]] || {
  echo "Missing cast: $CAST" >&2
  exit 1
}

set -a
source "$POC_OUTPUT/deployment.env"
set +a
[[ $("$CAST" chain-id --rpc-url "$RPC_URL") == 11155111 ]] || {
  echo "The prepared RPC is not Sepolia" >&2
  exit 1
}
[[ ${ETHEREUM_INDEXER_START_BLOCK:-} =~ ^[0-9]+$ ]] || {
  echo "Set ETHEREUM_INDEXER_START_BLOCK to the deployment block" >&2
  exit 1
}
[[ $(jq -r '.commitValidityPeriod' \
  "$FIXTURE_ROOT/bridge-scenario.json") == 2400 ]] || {
  echo "Testnet bridge fixtures must use a 2400-slot commit validity period" >&2
  exit 1
}

live_action_state=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'actionState()(bytes32)' --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]')
initial_action_state=$(jq -r '.outerActionStateBeforeDeposit | ascii_downcase' \
  "$FIXTURE_ROOT/bridge-scenario.json")
[[ $live_action_state == "$initial_action_state" ]] || {
  echo "Settlement action state does not match the OCaml bridge genesis" >&2
  exit 1
}

if [[ $CIRCUITS_CONFIG != "$TESTNET_DIR/config/circuits.json" ]]; then
  cp "$CIRCUITS_CONFIG" "$TESTNET_DIR/config/circuits.json"
fi
cp "$FIXTURE_ROOT/bridge-scenario.json" \
  "$TESTNET_DIR/config/bridge-scenario.json"
cp "$FIXTURE_ROOT/bridge-genesis-ledger.json" \
  "$TESTNET_DIR/config/bridge-genesis-ledger.json"
cp "$POC_OUTPUT/manifest.json" "$TESTNET_DIR/artifacts/manifest.json"

fixture="$FIXTURE_ROOT/deposit-sync/settlement.json"
outer_public_key=$(jq -r '.outerAccountPublicKey' "$fixture")
fee_payer_public_key=$(jq -r '.feePayerPublicKey' "$fixture")
fee_payer_nonce=$(jq -r '.nonce' "$fixture")
action_state_decimal=$("$CAST" to-dec "$initial_action_state")
jq -n --arg outerPublicKey "$outer_public_key" \
  --arg feePayerPublicKey "$fee_payer_public_key" \
  --arg nonce "$fee_payer_nonce" --arg actionState "$action_state_decimal" \
  'if $outerPublicKey == $feePayerPublicKey then
    [{publicKey:$outerPublicKey,tokenId:"1",nonce:$nonce,
      actionState:[$actionState,"0","0","0","0"]}]
   else
    [{publicKey:$outerPublicKey,tokenId:"1",
      actionState:[$actionState,"0","0","0","0"]},
     {publicKey:$feePayerPublicKey,tokenId:"1",nonce:$nonce}]
   end' >"$TESTNET_DIR/config/virtual-mina-accounts.json"

genesis_timestamp=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'genesisTimestamp()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
genesis_rfc3339=$(date -u -d "@$genesis_timestamp" +%Y-%m-%dT%H:%M:%SZ)
{
  echo "API_BIND=0.0.0.0:8080"
  echo "API_EXECUTE_ONLY=false"
  echo "API_LOCAL_MOCK_SUBMIT=false"
  echo "API_REQUIRE_PROOF_APPROVAL=true"
  echo "RPC_URL=$RPC_URL"
  echo "SETTLEMENT_CONTRACT_ADDRESS=$SETTLEMENT_CONTRACT_ADDRESS"
  echo "BRIDGE_CONTRACT_ADDRESS=$BRIDGE_CONTRACT_ADDRESS"
  echo "PROOF_SYSTEM=groth16"
  echo "PROVER_TIMEOUT_SECS=21600"
  echo "PROVER_MIN_AUCTION_PERIOD_SECS=15"
  echo "PROVER_MIN_REMAINING_SLOTS=1900"
  echo "PROVER_GAS_LIMIT=${PROVER_GAS_LIMIT:-}"
  echo "PROVER_MAX_PRICE_PER_PGU=${PROVER_MAX_PRICE_PER_PGU:-}"
  echo "PROVER_EXPLORER_BASE_URL=https://explorer.succinct.xyz/request"
  echo "ETHEREUM_CONFIRMATIONS=12"
  echo "ETHEREUM_POLL_INTERVAL_SECS=3"
  echo "ETHEREUM_INDEXER_START_BLOCK=$ETHEREUM_INDEXER_START_BLOCK"
  echo "VIRTUAL_MINA_GENESIS_TIMESTAMP=$genesis_rfc3339"
  echo "VIRTUAL_MINA_FORK_SLOT=$FORK_SLOT"
  echo "VIRTUAL_MINA_ACCOUNT_CREATION_FEE=1000000000"
  echo "VIRTUAL_MINA_INITIAL_STATE_HASH=0x0000000000000000000000000000000000000000000000000000000000000000"
  echo "VIRTUAL_MINA_ACCOUNTS_PATH=/config/virtual-mina-accounts.json"
  echo "VIRTUAL_MINA_OUTER_PUBLIC_KEY=$outer_public_key"
} >"$TESTNET_DIR/gateway.env"

da_public_keys=$(jq -r '.daPublicKeys | join(",")' \
  "$FIXTURE_ROOT/bridge-scenario.json")
sequencer_public_key=$(jq -r '.sequencerPublicKey' \
  "$FIXTURE_ROOT/bridge-scenario.json")
[[ -f "$TESTNET_DIR/.env" ]] || {
  echo "Initialize the retained machine identity before materializing config" >&2
  exit 1
}
awk -v da="$da_public_keys" -v sequencer="$sequencer_public_key" '
  /^DA_PUBLIC_KEYS=/ { print "DA_PUBLIC_KEYS=" da; next }
  /^SEQUENCER_PUBLIC_KEY=/ { print "SEQUENCER_PUBLIC_KEY=" sequencer; next }
  { print }
' "$TESTNET_DIR/.env" >"$TESTNET_DIR/.env.tmp"
mv "$TESTNET_DIR/.env.tmp" "$TESTNET_DIR/.env"
chmod 0600 "$TESTNET_DIR/.env"

jq -n --arg directory "$TESTNET_DIR" --arg daPublicKeys "$da_public_keys" \
  --arg sequencerPublicKey "$sequencer_public_key" \
  '{directory:$directory,daPublicKeys:$daPublicKeys,
    sequencerPublicKey:$sequencerPublicKey,
    minaSigningNetworkId:"testnet",
    next:"set proof cost hard caps, pin images, then run the testnet preflight"}'
