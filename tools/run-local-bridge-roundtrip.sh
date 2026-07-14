#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
FIXTURE_ROOT=${BRIDGE_FIXTURE_ROOT:-$ROOT/build/poc/bridge-fixtures}
DEPLOY_DIR=${POC_DEPLOY_DIR:-$ROOT/build/poc/bridge-roundtrip}
RPC_PORT=${RPC_PORT:-8547}
PG_PORT=${PG_PORT:-55432}
API_PORT=${API_PORT:-8081}
RPC_URL="http://127.0.0.1:$RPC_PORT"
API_URL="http://127.0.0.1:$API_PORT"
API_KEY=${PROOF_API_KEY:-local-bridge-roundtrip-key}
PRIVATE_KEY=${ETHEREUM_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}
ADMIN_ADDRESS=${ADMIN_ADDRESS:-0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266}
POSTGRES_CONTAINER=${POSTGRES_CONTAINER:-zeko-bridge-roundtrip-postgres-$$}
POSTGRES_IMAGE=${POSTGRES_IMAGE:-postgres:17-bookworm}
LOG_FILE=${POC_API_LOG:-/tmp/zeko-bridge-roundtrip-api-$$.log}
ANVIL_LOG=${POC_ANVIL_LOG:-/tmp/zeko-bridge-roundtrip-anvil-$$.log}
CAST=${CAST:-$HOME/.foundry/bin/cast}
FORGE=${FORGE:-$HOME/.foundry/bin/forge}
ANVIL=${ANVIL:-$HOME/.foundry/bin/anvil}
API_BIN=${API_BIN:-$ROOT/target/release/zeko-proof-api}

for command in bc curl docker jq date; do
  command -v "$command" >/dev/null || {
    echo "Missing command: $command" >&2
    exit 1
  }
done
for executable in "$CAST" "$FORGE" "$ANVIL"; do
  [[ -x "$executable" ]] || {
    echo "Missing executable: $executable" >&2
    exit 1
  }
done
for fixture in \
  "$FIXTURE_ROOT/bridge-scenario.json" \
  "$FIXTURE_ROOT/deposit-sync/settlement.json" \
  "$FIXTURE_ROOT/withdrawal/settlement.json"; do
  [[ -f "$fixture" ]] || {
    echo "Missing bridge fixture: $fixture" >&2
    exit 1
  }
done

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n ${API_PID:-} ]]; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  if [[ -n ${ANVIL_PID:-} ]]; then
    kill "$ANVIL_PID" 2>/dev/null || true
    wait "$ANVIL_PID" 2>/dev/null || true
  fi
  docker rm -f "$POSTGRES_CONTAINER" >/dev/null 2>&1 || true
  rm -f "${ACCOUNT_FILE:-}"
  if [[ $status -ne 0 ]]; then
    echo "Gateway log: $LOG_FILE" >&2
    echo "Anvil log: $ANVIL_LOG" >&2
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

if curl -fsS "$RPC_URL" >/dev/null 2>&1; then
  echo "RPC port $RPC_PORT is already in use" >&2
  exit 1
fi
if curl -fsS "$API_URL/health" >/dev/null 2>&1; then
  echo "API port $API_PORT is already in use" >&2
  exit 1
fi

"$ANVIL" --silent --chain-id 31337 --port "$RPC_PORT" >"$ANVIL_LOG" 2>&1 &
ANVIL_PID=$!
for _ in $(seq 1 30); do
  if [[ $("$CAST" chain-id --rpc-url "$RPC_URL" 2>/dev/null || true) == 31337 ]]; then
    break
  fi
  kill -0 "$ANVIL_PID" 2>/dev/null || exit 1
  sleep 1
done
[[ $("$CAST" chain-id --rpc-url "$RPC_URL") == 31337 ]]
# Keep pending-block timestamps deterministic while a Pickles execute spends
# tens of minutes on the CPU. Otherwise Anvil follows wall time and a narrow
# OCaml commit window can expire before the locally verified receipt is sent.
"$CAST" rpc anvil_setBlockTimestampInterval 1 --rpc-url "$RPC_URL" >/dev/null

rm -rf "$DEPLOY_DIR"
mkdir -p "$DEPLOY_DIR"
"$ROOT/tools/prepare-poc.sh" "$RPC_URL" "$ADMIN_ADDRESS" \
  "$FIXTURE_ROOT/deposit-sync" "$DEPLOY_DIR"
set -a
source "$DEPLOY_DIR/deployment.env"
set +a
expected_bridge=$(jq -r '.proof.innerActionBatch.bridgeAddress | ascii_downcase' \
  "$FIXTURE_ROOT/deposit-sync/settlement.json")
[[ ${BRIDGE_CONTRACT_ADDRESS,,} == "$expected_bridge" ]] || {
  echo "Fresh deterministic bridge address does not match the OCaml circuit" >&2
  exit 1
}

(
  cd "$ROOT/contracts"
  PRIVATE_KEY="$PRIVATE_KEY" LOCAL_MOCK_VERIFIER=true \
    "$FORGE" script script/DeployPoc.s.sol:DeployPoc \
      --rpc-url "$RPC_URL" --broadcast >/dev/null
)
[[ $("$CAST" call "$LOCAL_SP1_VERIFIER_ADDRESS" \
  'isLocalSP1Verifier()(bool)' --rpc-url "$RPC_URL") == true ]]
[[ $("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'actionState()(bytes32)' --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]') == \
  "$(jq -r '.outerActionStateBeforeDeposit | ascii_downcase' \
    "$FIXTURE_ROOT/bridge-scenario.json")" ]]

docker run -d --name "$POSTGRES_CONTAINER" \
  -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=zeko_proofs \
  -p "127.0.0.1:$PG_PORT:5432" "$POSTGRES_IMAGE" >/dev/null
for _ in $(seq 1 30); do
  if docker exec "$POSTGRES_CONTAINER" pg_isready -U postgres -d zeko_proofs \
      >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec "$POSTGRES_CONTAINER" pg_isready -U postgres -d zeko_proofs \
  >/dev/null

outer_public_key=$(jq -r '.outerAccountPublicKey' \
  "$FIXTURE_ROOT/deposit-sync/settlement.json")
fee_payer_public_key=$(jq -r '.feePayerPublicKey' \
  "$FIXTURE_ROOT/deposit-sync/settlement.json")
fee_payer_nonce=$(jq -r '.nonce' \
  "$FIXTURE_ROOT/deposit-sync/settlement.json")
initial_action_state=$(jq -r '.outerActionStateBeforeDeposit' \
  "$FIXTURE_ROOT/bridge-scenario.json")
initial_action_state_decimal=$("$CAST" to-dec "$initial_action_state")
ACCOUNT_FILE=$(mktemp)
jq -n --arg outerPublicKey "$outer_public_key" \
  --arg feePayerPublicKey "$fee_payer_public_key" \
  --arg nonce "$fee_payer_nonce" --arg actionState "$initial_action_state_decimal" \
  'if $outerPublicKey == $feePayerPublicKey then
    [{publicKey:$outerPublicKey,tokenId:"1",nonce:$nonce,
      actionState:[$actionState,"0","0","0","0"]}]
   else
    [{publicKey:$outerPublicKey,tokenId:"1",
      actionState:[$actionState,"0","0","0","0"]},
     {publicKey:$feePayerPublicKey,tokenId:"1",nonce:$nonce}]
   end' >"$ACCOUNT_FILE"

genesis_timestamp=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'genesisTimestamp()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
export DATABASE_URL="postgres://postgres:postgres@127.0.0.1:$PG_PORT/zeko_proofs"
export PROOF_API_KEY="$API_KEY"
export API_BIND="127.0.0.1:$API_PORT"
export API_EXECUTE_ONLY=false
export API_LOCAL_MOCK_SUBMIT=true
export API_REQUIRE_PROOF_APPROVAL=false
export ETHEREUM_PRIVATE_KEY="$PRIVATE_KEY"
export ETHEREUM_CONFIRMATIONS=1
export ETHEREUM_POLL_INTERVAL_SECS=1
export ETHEREUM_INDEXER_START_BLOCK=0
VIRTUAL_MINA_GENESIS_TIMESTAMP=$(date -u -d "@$genesis_timestamp" +%Y-%m-%dT%H:%M:%SZ)
export VIRTUAL_MINA_GENESIS_TIMESTAMP
export VIRTUAL_MINA_FORK_SLOT="$FORK_SLOT"
export VIRTUAL_MINA_OUTER_PUBLIC_KEY="$outer_public_key"
export VIRTUAL_MINA_ACCOUNTS_PATH="$ACCOUNT_FILE"

[[ -x "$API_BIN" ]] || {
  echo "Gateway binary was not built by prepare-poc: $API_BIN" >&2
  exit 1
}
"$API_BIN" >"$LOG_FILE" 2>&1 &
API_PID=$!
for _ in $(seq 1 60); do
  if curl -fsS "$API_URL/health" >/dev/null 2>&1; then
    break
  fi
  kill -0 "$API_PID" 2>/dev/null || exit 1
  sleep 1
done
curl -fsS "$API_URL/health" >/dev/null

wait_job() {
  local id=$1
  local response status
  for _ in $(seq 1 7200); do
    response=$(curl -fsS -H "x-api-key: $API_KEY" \
      "$API_URL/v1/proofs/$id" 2>/dev/null || true)
    status=$(jq -r '.status // empty' <<<"$response")
    case "$status" in
      confirmed)
        jq '{id,kind,status,cycleCount,transactionHash,ethereumGasUsed}' \
          <<<"$response"
        return 0
        ;;
      failed|rejected|reorged)
        jq '{id,kind,status,error,cycleCount,transactionHash}' <<<"$response" >&2
        return 1
        ;;
      submitted)
        "$CAST" rpc anvil_mine 0x1 --rpc-url "$RPC_URL" >/dev/null
        ;;
    esac
    sleep 1
  done
  echo "Proof job $id timed out" >&2
  return 1
}

submit_settlement() {
  local fixture=$1
  local lower_slot current_slot genesis_timestamp slot_duration target_timestamp
  lower_slot=$("$CAST" to-dec "$(jq -er '.proof.binding.actions[0][6]' "$fixture")")
  current_slot=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
    'currentVirtualSlot()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
  if ((current_slot < lower_slot)); then
    genesis_timestamp=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
      'genesisTimestamp()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
    slot_duration=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
      'slotDuration()(uint32)' --rpc-url "$RPC_URL" | awk '{print $1}')
    target_timestamp=$((genesis_timestamp + (lower_slot - FORK_SLOT) * slot_duration))
    "$CAST" rpc evm_setNextBlockTimestamp "$target_timestamp" \
      --rpc-url "$RPC_URL" >/dev/null
    "$CAST" rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
  fi
  jq -n --slurpfile settlement "$fixture" --arg token "$API_KEY" \
    '{query:"mutation { sendZkapp { zkapp { id failureReason } } }",
      variables:{gatewayToken:$token,settlement:$settlement[0]}}' \
  | curl -fsS -H 'content-type: application/json' --data-binary @- \
      "$API_URL/graphql" \
  | jq -e '.data.sendZkapp.zkapp.failureReason == []' >/dev/null
  local id=
  for _ in $(seq 1 30); do
    id=$(curl -fsS -H "x-api-key: $API_KEY" \
      "$API_URL/v1/proofs?kind=settlement&limit=1" \
      | jq -r '.[0].id // empty')
    [[ -n $id ]] && break
    sleep 1
  done
  [[ -n $id ]]
  wait_job "$id"
}

zeko_recipient=$(jq -r '.zekoRecipient' "$FIXTURE_ROOT/bridge-scenario.json")
deposit_amount=$(jq -r '.depositAmountZeko' "$FIXTURE_ROOT/bridge-scenario.json")
((deposit_amount % 1000000000 == 0))
deposit_eth=$((deposit_amount / 1000000000))
"$CAST" send "$BRIDGE_CONTRACT_ADDRESS" 'depositETH(uint256)' "$zeko_recipient" \
  --value "${deposit_eth}ether" --private-key "$PRIVATE_KEY" \
  --rpc-url "$RPC_URL" >/dev/null
"$CAST" rpc anvil_mine 0x2 --rpc-url "$RPC_URL" >/dev/null

for _ in $(seq 1 60); do
  deposit_status=$(curl -fsS "$API_URL/v1/bridge/deposits/1" 2>/dev/null \
    | jq -r '.status // empty' || true)
  [[ $deposit_status == locked ]] && break
  sleep 1
done
[[ ${deposit_status:-} == locked ]]
bridge_job=$(curl -fsS -H "x-api-key: $API_KEY" -X POST \
  "$API_URL/v1/bridge/deposits/prove" | jq -r '.id')
wait_job "$bridge_job"
[[ $("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" 'actionState()(bytes32)' \
  --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]') == \
  "$(jq -r '.outerActionStateAfterDeposit | ascii_downcase' \
    "$FIXTURE_ROOT/bridge-scenario.json")" ]]

submit_settlement "$FIXTURE_ROOT/deposit-sync/settlement.json"
for _ in $(seq 1 60); do
  deposit_status=$(curl -fsS "$API_URL/v1/bridge/deposits/1" \
    | jq -r '.status')
  [[ $deposit_status == synchronized ]] && break
  sleep 1
done
[[ $deposit_status == synchronized ]]

submit_settlement "$FIXTURE_ROOT/withdrawal/settlement.json"
withdrawal_recipient=$(jq -r '.withdrawalRecipient' \
  "$FIXTURE_ROOT/bridge-scenario.json")
withdrawal=$(curl -fsS \
  "$API_URL/v1/bridge/withdrawals?recipient=$withdrawal_recipient" \
  | jq -e '.[0]')
claimable_slot=$(jq -r '.claimableSlot' <<<"$withdrawal")
genesis_timestamp=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'genesisTimestamp()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
slot_duration=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'slotDuration()(uint32)' --rpc-url "$RPC_URL" | awk '{print $1}')
target_timestamp=$((genesis_timestamp + (claimable_slot - FORK_SLOT + 1) * slot_duration))
current_slot=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'currentVirtualSlot()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
if ((current_slot <= claimable_slot)); then
  "$CAST" rpc evm_setNextBlockTimestamp "$target_timestamp" \
    --rpc-url "$RPC_URL" >/dev/null
  "$CAST" rpc evm_mine --rpc-url "$RPC_URL" >/dev/null
fi
withdrawal=$(curl -fsS \
  "$API_URL/v1/bridge/withdrawals?recipient=$withdrawal_recipient" \
  | jq -e '.[0]')
[[ $(jq -r '.status' <<<"$withdrawal") == claimable ]]

sequence=$(jq -r '.settlementSequence' <<<"$withdrawal")
offset=$(jq -r '.offset' <<<"$withdrawal")
amount=$(jq -r '.amount' <<<"$withdrawal")
action_fields_hash=$(jq -r '.actionFieldsHash' <<<"$withdrawal")
proof=$(jq -r '.siblings | "[" + join(",") + "]"' <<<"$withdrawal")
liability_before=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'nativeEscrowLiability()(uint256)' --rpc-url "$RPC_URL" | awk '{print $1}')
"$CAST" send "$BRIDGE_CONTRACT_ADDRESS" \
  'claimNativeWithdrawal(uint64,uint32,address,uint64,bytes32,bytes32[16])' \
  "$sequence" "$offset" "$withdrawal_recipient" "$amount" \
  "$action_fields_hash" "$proof" --private-key "$PRIVATE_KEY" \
  --rpc-url "$RPC_URL" >/dev/null
liability_after=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'nativeEscrowLiability()(uint256)' --rpc-url "$RPC_URL" | awk '{print $1}')
expected_wei=$((amount * 1000000000))
released_wei=$(bc <<<"$liability_before - $liability_after")
[[ $released_wei == "$expected_wei" ]]

jq -n --arg bridge "$BRIDGE_CONTRACT_ADDRESS" \
  --arg settlement "$SETTLEMENT_CONTRACT_ADDRESS" \
  --arg recipient "$withdrawal_recipient" --arg amountZeko "$amount" \
  '{status:"passed",bridge:$bridge,settlement:$settlement,
    withdrawalRecipient:$recipient,withdrawalAmountZeko:$amountZeko,
    ocamlCommits:2,sp1ProofsGenerated:0}'
echo "ETH deposit -> bridge execute -> two real OCaml settlements -> Merkle withdrawal claim passed."
echo "No SP1 proof was requested or generated."
