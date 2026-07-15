#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ENV_FILE=${POC_ENV_FILE:-$ROOT/build/poc/deployment.env}
FIXTURE=${SETTLEMENT_FIXTURE:-$ROOT/fixtures/zeko-local-e2e/settlement.json}
API_BIN=${API_BIN:-$ROOT/target/release/zeko-proof-api}
API_PORT=${API_PORT:-8080}
POSTGRES_CONTAINER=${POSTGRES_CONTAINER:-zeko-poc-postgres}
DATABASE_NAME=${DATABASE_NAME:-zeko_settlement_e2e_$$}
API_KEY=${PROOF_API_KEY:-local-e2e-key}
PRIVATE_KEY=${ETHEREUM_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}
LOG_FILE=${POC_API_LOG:-/tmp/zeko-settlement-api-$$.log}
CAST=${CAST:-$HOME/.foundry/bin/cast}

[[ -f "$ENV_FILE" && -f "$FIXTURE" && -x "$API_BIN" && -x "$CAST" ]] || {
  echo "Missing deployment env, settlement fixture, gateway, or cast" >&2
  exit 1
}
set -a
source "$ENV_FILE"
set +a

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n ${API_PID:-} ]]; then
    kill "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
  fi
  docker exec "$POSTGRES_CONTAINER" psql -U postgres -d postgres \
    -c "DROP DATABASE IF EXISTS \"$DATABASE_NAME\" WITH (FORCE);" \
    >/dev/null 2>&1 || true
  rm -f "${ACCOUNT_FILE:-}"
  if [[ $status -ne 0 ]]; then
    echo "Gateway log: $LOG_FILE" >&2
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

[[ $("$CAST" chain-id --rpc-url "$RPC_URL") == 31337 ]]
[[ $("$CAST" call "$LOCAL_SP1_VERIFIER_ADDRESS" \
  'isLocalSP1Verifier()(bool)' --rpc-url "$RPC_URL") == true ]]
batch_before=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'batchSequence()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
[[ $batch_before =~ ^[0-9]+$ ]] || {
  echo "Invalid batch sequence returned by cast: $batch_before" >&2
  exit 1
}
if [[ -n ${EXPECTED_BATCH_SEQUENCE:-} && $batch_before != "$EXPECTED_BATCH_SEQUENCE" ]]; then
  echo "Expected batch $EXPECTED_BATCH_SEQUENCE, live contract is $batch_before" >&2
  exit 1
fi
current_slot=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'currentVirtualSlot()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
[[ $current_slot =~ ^[0-9]+$ ]] || {
  echo "Invalid virtual slot returned by cast: $current_slot" >&2
  exit 1
}
slot_lower=$("$CAST" to-dec "$(jq -r '.proof.binding.actions[0][6]' "$FIXTURE")")
slot_upper=$("$CAST" to-dec "$(jq -r '.proof.binding.actions[0][7]' "$FIXTURE")")
if ((current_slot < slot_lower || current_slot > slot_upper)); then
  echo "Fixture slot range [$slot_lower, $slot_upper] does not include live slot $current_slot" >&2
  exit 1
fi

docker exec "$POSTGRES_CONTAINER" psql -U postgres -d postgres \
  -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$DATABASE_NAME\";" >/dev/null

outer_public_key=$(jq -r '.outerAccountPublicKey' "$FIXTURE")
fee_payer_public_key=$(jq -r '.feePayerPublicKey' "$FIXTURE")
fee_payer_nonce=$(jq -r '.nonce' "$FIXTURE")
outer_action_state=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'actionState()(bytes32)' --rpc-url "$RPC_URL")
outer_action_state_decimal=$("$CAST" to-dec "$outer_action_state")
ACCOUNT_FILE=$(mktemp)
jq -n --arg outerPublicKey "$outer_public_key" \
  --arg feePayerPublicKey "$fee_payer_public_key" \
  --arg nonce "$fee_payer_nonce" --arg actionState "$outer_action_state_decimal" \
  'if $outerPublicKey == $feePayerPublicKey then
    [{publicKey:$outerPublicKey,tokenId:"1",nonce:$nonce,
      actionState:[$actionState,"0","0","0","0"]}]
   else
    [{publicKey:$outerPublicKey,tokenId:"1",
      actionState:[$actionState,"0","0","0","0"]},
     {publicKey:$feePayerPublicKey,tokenId:"1",nonce:$nonce}]
   end' \
  >"$ACCOUNT_FILE"

export DATABASE_URL="postgres://postgres:postgres@127.0.0.1:5432/$DATABASE_NAME"
export PROOF_API_KEY="$API_KEY"
export API_BIND="127.0.0.1:$API_PORT"
export API_EXECUTE_ONLY=false
export API_LOCAL_MOCK_SUBMIT=true
export ETHEREUM_PRIVATE_KEY="$PRIVATE_KEY"
export ETHEREUM_FINALITY_MODE=confirmations
export ETHEREUM_CONFIRMATIONS=1
export ETHEREUM_POLL_INTERVAL_SECS=1
export ETHEREUM_INDEXER_START_BLOCK=0
export VIRTUAL_MINA_GENESIS_TIMESTAMP=2026-01-01T00:00:00Z
export VIRTUAL_MINA_FORK_SLOT=${FORK_SLOT:-0}
export VIRTUAL_MINA_OUTER_PUBLIC_KEY="$outer_public_key"
export VIRTUAL_MINA_ACCOUNTS_PATH="$ACCOUNT_FILE"

if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then
  echo "Gateway port $API_PORT is already serving a health endpoint" >&2
  exit 1
fi
"$API_BIN" >"$LOG_FILE" 2>&1 &
API_PID=$!
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then
    break
  fi
  kill -0 "$API_PID" 2>/dev/null || exit 1
  sleep 1
done
curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null

jq -n --slurpfile settlement "$FIXTURE" --arg token "$API_KEY" \
  '{query:"mutation { sendZkapp { zkapp { id failureReason } } }",
    variables:{gatewayToken:$token,settlement:$settlement[0]}}' \
  | curl -fsS -H 'content-type: application/json' --data-binary @- \
      "http://127.0.0.1:$API_PORT/graphql" \
  | jq -e '.data.sendZkapp.zkapp.failureReason == []' >/dev/null

for _ in $(seq 1 30); do
  job=$(curl -fsS -H "x-api-key: $API_KEY" \
    "http://127.0.0.1:$API_PORT/v1/proofs?kind=settlement&limit=1" \
    | jq -r '.[0].id // empty')
  [[ -n $job ]] && break
  sleep 1
done
[[ -n ${job:-} ]]

for _ in $(seq 1 3600); do
  if ! response=$(curl -fsS -H "x-api-key: $API_KEY" \
    "http://127.0.0.1:$API_PORT/v1/proofs/$job" 2>/dev/null); then
    sleep 1
    continue
  fi
  status=$(jq -r '.status' <<<"$response")
  case "$status" in
    confirmed) break ;;
    failed)
      jq '{id,kind,status,error,cycleCount,transactionHash}' <<<"$response" >&2
      exit 1
      ;;
    submitted)
      "$CAST" rpc anvil_mine 0x1 --rpc-url "$RPC_URL" >/dev/null
      ;;
  esac
  sleep 1
done
[[ ${status:-} == confirmed ]]
batch_after=$((batch_before + 1))
[[ $("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'batchSequence()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}') == "$batch_after" ]]

jq '{job: .id, status, cycleCount, transactionHash, ethereumGasUsed}' \
  <<<"$response"
echo "OCaml Pickles fixture -> SP1 execute -> settlement transition passed."
echo "No SP1 proof was generated."
