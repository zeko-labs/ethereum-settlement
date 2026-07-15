#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ENV_FILE=${POC_ENV_FILE:-$ROOT/build/poc/deployment.env}
API_BIN=${API_BIN:-$ROOT/target/release/zeko-proof-api}
API_PORT=${API_PORT:-8080}
POSTGRES_CONTAINER=${POSTGRES_CONTAINER:-zeko-poc-postgres}
DATABASE_NAME=${DATABASE_NAME:-zeko_poc_e2e_$$}
API_KEY=${PROOF_API_KEY:-local-e2e-key}
PRIVATE_KEY=${ETHEREUM_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}
LOG_FILE=${POC_API_LOG:-/tmp/zeko-poc-api-$$.log}

[[ -f "$ENV_FILE" ]] || {
  echo "Missing deployment environment: $ENV_FILE" >&2
  exit 1
}
[[ -x "$API_BIN" ]] || {
  echo "Missing gateway binary: $API_BIN" >&2
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

for command in curl docker jq; do
  command -v "$command" >/dev/null || {
    echo "Missing command: $command" >&2
    exit 1
  }
done
CAST=${CAST:-$HOME/.foundry/bin/cast}
[[ -x "$CAST" ]] || {
  echo "Missing cast binary: $CAST" >&2
  exit 1
}

[[ $("$CAST" chain-id --rpc-url "$RPC_URL") == 31337 ]] || {
  echo "Local mock E2E requires chain ID 31337" >&2
  exit 1
}
[[ $("$CAST" call "$LOCAL_SP1_VERIFIER_ADDRESS" \
  'isLocalSP1Verifier()(bool)' --rpc-url "$RPC_URL") == true ]] || {
  echo "The deterministic LocalSP1Verifier is not deployed" >&2
  exit 1
}

docker exec "$POSTGRES_CONTAINER" psql -U postgres -d postgres \
  -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$DATABASE_NAME\";" >/dev/null

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
export VIRTUAL_MINA_OUTER_PUBLIC_KEY=${VIRTUAL_MINA_OUTER_PUBLIC_KEY:-$(
  jq -r '.outerAccountPublicKey' "$ROOT/fixtures/zeko-local-e2e/settlement.json"
)}
ACCOUNT_FILE=$(mktemp)
outer_action_state=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'actionState()(bytes32)' --rpc-url "$RPC_URL")
outer_action_state_decimal=$("$CAST" to-dec "$outer_action_state")
jq -n --arg publicKey "$VIRTUAL_MINA_OUTER_PUBLIC_KEY" \
  --arg actionState "$outer_action_state_decimal" \
  '[{publicKey:$publicKey,tokenId:"1",actionState:[$actionState,"0","0","0","0"]}]' \
  >"$ACCOUNT_FILE"
export VIRTUAL_MINA_ACCOUNTS_PATH="$ACCOUNT_FILE"

if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1; then
  echo "Gateway port $API_PORT is already serving a health endpoint" >&2
  exit 1
fi
"$API_BIN" >"$LOG_FILE" 2>&1 &
API_PID=$!
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null; then
    break
  fi
  kill -0 "$API_PID" 2>/dev/null || {
    echo "Gateway exited during startup" >&2
    exit 1
  }
  sleep 1
done
curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null

before=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'depositNonce()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
[[ $before =~ ^[0-9]+$ ]] || {
  echo "Invalid deposit nonce returned by cast: $before" >&2
  exit 1
}
nonce=$((before + 1))
"$CAST" send "$BRIDGE_CONTRACT_ADDRESS" 'depositETH(uint256)' "$nonce" \
  --value 1ether --private-key "$PRIVATE_KEY" --rpc-url "$RPC_URL" \
  >/dev/null
"$CAST" rpc anvil_mine 0x2 --rpc-url "$RPC_URL" >/dev/null

for _ in $(seq 1 30); do
  status=$(curl -fsS "http://127.0.0.1:$API_PORT/v1/bridge/deposits/$nonce" \
    2>/dev/null | jq -r '.status' || true)
  [[ $status == locked ]] && break
  sleep 1
done
[[ ${status:-} == locked ]] || {
  echo "Deposit was not indexed and finalized" >&2
  exit 1
}

job=$(curl -fsS -H "x-api-key: $API_KEY" -X POST \
  "http://127.0.0.1:$API_PORT/v1/bridge/deposits/prove" | jq -r '.id')
for _ in $(seq 1 120); do
  if ! response=$(curl -fsS -H "x-api-key: $API_KEY" \
    "http://127.0.0.1:$API_PORT/v1/proofs/$job" 2>/dev/null); then
    sleep 1
    continue
  fi
  job_status=$(jq -r '.status' <<<"$response")
  case "$job_status" in
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
[[ ${job_status:-} == confirmed ]] || {
  echo "Bridge job did not confirm" >&2
  exit 1
}

deposit=$(curl -fsS "http://127.0.0.1:$API_PORT/v1/bridge/deposits/$nonce")
[[ $(jq -r '.status' <<<"$deposit") == bridgeProven ]]
[[ $(jq -r '.outerActionSequence' <<<"$deposit") != null ]]
[[ $("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'bridgedDepositNonce()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}') == "$nonce" ]]

jq '{job: .id, status, cycleCount, transactionHash}' <<<"$response"
jq '{nonce, status, nextAction, outerActionSequence, outerActionStateAfter}' \
  <<<"$deposit"
echo "Local deposit -> SP1 execute -> contract transition passed."
echo "No SP1 proof was generated."
