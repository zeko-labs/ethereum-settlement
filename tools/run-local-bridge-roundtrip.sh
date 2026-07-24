#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/tools/lib/workspace.sh"
zeko_resolve_companion_repo "$ROOT" ZEKO_UI_ROOT zeko-ui packages/eth-bridge-sdk
BRIDGE_ASSET=${BRIDGE_ASSET:-native}
case "$BRIDGE_ASSET" in
native)
  DEFAULT_FIXTURE_ROOT=$ROOT/build/poc/bridge-fixtures
  DEFAULT_DEPLOY_DIR=$ROOT/build/poc/bridge-roundtrip
  ;;
erc20)
  DEFAULT_FIXTURE_ROOT=$ROOT/build/poc/bridge-erc20-fixtures
  DEFAULT_DEPLOY_DIR=$ROOT/build/poc/bridge-erc20-roundtrip
  ;;
*)
  echo "Unsupported BRIDGE_ASSET: $BRIDGE_ASSET" >&2
  exit 1
  ;;
esac
FIXTURE_ROOT=${BRIDGE_FIXTURE_ROOT:-$DEFAULT_FIXTURE_ROOT}
DEPLOY_DIR=${POC_DEPLOY_DIR:-$DEFAULT_DEPLOY_DIR}
RPC_PORT=${RPC_PORT:-8547}
PG_PORT=${PG_PORT:-55432}
API_PORT=${API_PORT:-8081}
ACTIONS_INDEXER_PORT=${ACTIONS_INDEXER_PORT:-3601}
ACTIONS_API_PORT=${ACTIONS_API_PORT:-9101}
RPC_URL="http://127.0.0.1:$RPC_PORT"
API_URL="http://127.0.0.1:$API_PORT"
API_KEY=${PROOF_API_KEY:-local-bridge-roundtrip-key}
PRIVATE_KEY=${ETHEREUM_PRIVATE_KEY:-0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80}
ADMIN_ADDRESS=${ADMIN_ADDRESS:-0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266}
POSTGRES_CONTAINER=${POSTGRES_CONTAINER:-zeko-bridge-roundtrip-postgres-$$}
POSTGRES_IMAGE=${POSTGRES_IMAGE:-postgres:17-bookworm}
LOG_FILE=${POC_API_LOG:-/tmp/zeko-bridge-roundtrip-api-$$.log}
ANVIL_LOG=${POC_ANVIL_LOG:-/tmp/zeko-bridge-roundtrip-anvil-$$.log}
ACTIONS_INDEXER_LOG=${POC_ACTIONS_INDEXER_LOG:-/tmp/zeko-bridge-roundtrip-actions-indexer-$$.log}
ACTIONS_API_LOG=${POC_ACTIONS_API_LOG:-/tmp/zeko-bridge-roundtrip-actions-api-$$.log}
CAST=${CAST:-$HOME/.foundry/bin/cast}
FORGE=${FORGE:-$HOME/.foundry/bin/forge}
ANVIL=${ANVIL:-$HOME/.foundry/bin/anvil}
API_BIN=${API_BIN:-$ROOT/target/release/zeko-proof-api}
NIX=${NIX:-$HOME/.nix-profile/bin/nix}

for command in bc curl docker jq date pgrep; do
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
private_key_address=$(
  "$CAST" wallet address --private-key "$PRIVATE_KEY"
)
[[ ${private_key_address,,} == "${ADMIN_ADDRESS,,}" ]] || {
  echo "ETHEREUM_PRIVATE_KEY must belong to ADMIN_ADDRESS" >&2
  exit 1
}
[[ -x "$NIX" ]] || {
  echo "Missing Nix executable: $NIX" >&2
  exit 1
}
[[ -f "$ZEKO_UI_ROOT/packages/eth-bridge-sdk/package.json" ]] || {
  echo "Missing Ethereum bridge SDK checkout: $ZEKO_UI_ROOT" >&2
  exit 1
}
required_fixtures=(
  "$FIXTURE_ROOT/bridge-scenario.json"
  "$FIXTURE_ROOT/deposit-sync/settlement.json"
  "$FIXTURE_ROOT/withdrawal/settlement.json"
)
if [[ $BRIDGE_ASSET == erc20 ]]; then
  required_fixtures+=("$FIXTURE_ROOT/registration/settlement.json")
fi
for fixture in "${required_fixtures[@]}"; do
  [[ -f "$fixture" ]] || {
    echo "Missing bridge fixture: $fixture" >&2
    exit 1
  }
done
[[ $(jq -r '.bridgeAsset // "native"' "$FIXTURE_ROOT/bridge-scenario.json") == \
  "$BRIDGE_ASSET" ]] || {
  echo "Bridge fixture asset does not match BRIDGE_ASSET=$BRIDGE_ASSET" >&2
  exit 1
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n ${ACTIONS_INDEXER_PID:-} ]]; then
    terminate_tree "$ACTIONS_INDEXER_PID"
  fi
  if [[ -n ${ACTIONS_API_PID:-} ]]; then
    terminate_tree "$ACTIONS_API_PID"
  fi
  if [[ -n ${API_PID:-} ]]; then
    terminate_tree "$API_PID"
  fi
  if [[ -n ${ANVIL_PID:-} ]]; then
    terminate_tree "$ANVIL_PID"
  fi
  docker rm -f "$POSTGRES_CONTAINER" >/dev/null 2>&1 || true
  rm -f "${ACCOUNT_FILE:-}"
  rm -f "${SDK_OUTPUT:-}"
  rm -f "${ASSET_BATCH_OUTPUT:-}"
  if [[ $status -ne 0 ]]; then
    echo "Gateway log: $LOG_FILE" >&2
    echo "Anvil log: $ANVIL_LOG" >&2
    echo "Actions indexer log: $ACTIONS_INDEXER_LOG" >&2
    echo "Actions API log: $ACTIONS_API_LOG" >&2
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

terminate_tree() {
  local pid=$1 child
  while read -r child; do
    [[ -n $child ]] && terminate_tree "$child"
  done < <(pgrep -P "$pid" 2>/dev/null || true)
  kill "$pid" 2>/dev/null || true
  for _ in $(seq 1 20); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  kill -9 "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

if curl -fsS "$RPC_URL" >/dev/null 2>&1; then
  echo "RPC port $RPC_PORT is already in use" >&2
  exit 1
fi
if curl -fsS "$API_URL/health" >/dev/null 2>&1; then
  echo "API port $API_PORT is already in use" >&2
  exit 1
fi
for port in "$ACTIONS_INDEXER_PORT" "$ACTIONS_API_PORT"; do
  if curl -fsS "http://127.0.0.1:$port" >/dev/null 2>&1; then
    echo "Actions service port $port is already in use" >&2
    exit 1
  fi
done

# The browser SDK intentionally delegates signing to its EIP-1193 provider.
# Let the local node impersonate the retained fixture account so the same path
# works with generated testnet identities instead of only Anvil's default key.
"$ANVIL" --silent --auto-impersonate --chain-id 31337 --port "$RPC_PORT" \
  >"$ANVIL_LOG" 2>&1 &
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
# The retained testnet fixture is compiled against its generated admin's
# deterministic bridge address rather than Anvil's built-in first account.
# Fund whichever fixture admin was selected so the same runner handles both.
"$CAST" rpc anvil_setBalance "$ADMIN_ADDRESS" 0x21e19e0c9bab2400000 \
  --rpc-url "$RPC_URL" >/dev/null

rm -rf "$DEPLOY_DIR"
mkdir -p "$DEPLOY_DIR"
initial_fixture_dir="$FIXTURE_ROOT/deposit-sync"
initial_action_state_field=outerActionStateBeforeDeposit
if [[ $BRIDGE_ASSET == erc20 ]]; then
  initial_fixture_dir="$FIXTURE_ROOT/registration"
  initial_action_state_field=outerActionStateBeforeRegistration
fi
"$ROOT/tools/prepare-poc.sh" "$RPC_URL" "$ADMIN_ADDRESS" \
  "$initial_fixture_dir" "$DEPLOY_DIR"
set -a
source "$DEPLOY_DIR/deployment.env"
set +a
expected_bridge=$(jq -r '.proof.innerActionBatch.bridgeAddress | ascii_downcase' \
  "$FIXTURE_ROOT/deposit-sync/settlement.json")
[[ ${BRIDGE_CONTRACT_ADDRESS,,} == "$expected_bridge" ]] || {
  echo "Fresh deterministic bridge address does not match the OCaml circuit" >&2
  exit 1
}
if [[ $BRIDGE_ASSET == erc20 ]]; then
  export ERC20_TOKEN_ENABLED=true
  export ERC20_REGISTRY_L2 ERC20_SHARED_VAULT_L2
  export ERC20_SHARED_VAULT_PACKED ERC20_MFT_STANDARD_VK_ID
  export ERC20_UNIVERSAL_BRIDGE_VK_ID ERC20_REGISTRY_SCHEMA_VERSION
  ERC20_REGISTRY_L2=$(jq -er '.ethereumAssetRegistryL2' \
    "$FIXTURE_ROOT/bridge-scenario.json")
  ERC20_SHARED_VAULT_L2=$(jq -er '.ethereumSharedVaultL2' \
    "$FIXTURE_ROOT/bridge-scenario.json")
  ERC20_SHARED_VAULT_PACKED=$(jq -er '.ethereumAssets[0].record.vaultPublicKey' \
    "$FIXTURE_ROOT/bridge-scenario.json")
  ERC20_MFT_STANDARD_VK_ID=$(jq -er '.ethereumAssets[0].record.mftStandardVkId' \
    "$FIXTURE_ROOT/bridge-scenario.json")
  ERC20_UNIVERSAL_BRIDGE_VK_ID=$(jq -er \
    '.ethereumAssets[0].record.universalBridgeVkId' \
    "$FIXTURE_ROOT/bridge-scenario.json")
  ERC20_REGISTRY_SCHEMA_VERSION=1
  for index in 0 1; do
    prefix="ERC20_TOKEN_${index}_"
    printf -v "${prefix}ADDRESS" '%s' "$(jq -er --argjson index "$index" \
      '.ethereumAssets[$index].record.ethereumToken | ascii_downcase' \
      "$FIXTURE_ROOT/bridge-scenario.json")"
    printf -v "${prefix}ASSET_ID" '%s' "$(jq -er --argjson index "$index" \
      '.ethereumAssets[$index].record.assetId | ascii_downcase' \
      "$FIXTURE_ROOT/bridge-scenario.json")"
    printf -v "${prefix}OWNER_PACKED" '%s' "$(jq -er --argjson index "$index" \
      '.ethereumAssets[$index].record.tokenOwnerL2' \
      "$FIXTURE_ROOT/bridge-scenario.json")"
    printf -v "${prefix}TOKEN_ID" '%s' "$(jq -er --argjson index "$index" \
      '.ethereumAssets[$index].record.tokenIdL2' \
      "$FIXTURE_ROOT/bridge-scenario.json")"
    printf -v "${prefix}DEPOSIT_CAP" '%s' "$(jq -er --argjson index "$index" \
      '.ethereumAssets[$index].record.inventoryCap' \
      "$FIXTURE_ROOT/bridge-scenario.json")"
    printf -v "${prefix}DEPOSIT_AMOUNT" '%s' "$(jq -er '.depositAmountZeko' \
      "$FIXTURE_ROOT/bridge-scenario.json")"
    export "${prefix}ADDRESS" "${prefix}ASSET_ID" "${prefix}OWNER_PACKED"
    export "${prefix}TOKEN_ID" "${prefix}DEPOSIT_CAP" "${prefix}DEPOSIT_AMOUNT"
  done
fi

(
  cd "$ROOT/contracts"
  PRIVATE_KEY="$PRIVATE_KEY" LOCAL_MOCK_VERIFIER=true \
    "$FORGE" script script/DeployPoc.s.sol:DeployPoc \
      --rpc-url "$RPC_URL" --broadcast --slow >/dev/null
)
[[ $("$CAST" call "$LOCAL_SP1_VERIFIER_ADDRESS" \
  'isLocalSP1Verifier()(bool)' --rpc-url "$RPC_URL") == true ]]
expected_initial_action_state=$(jq -r \
  ".$initial_action_state_field | ascii_downcase" \
  "$FIXTURE_ROOT/bridge-scenario.json")
[[ $("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'actionState()(bytes32)' --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]') == \
  "$expected_initial_action_state" ]]
if [[ $BRIDGE_ASSET == erc20 ]]; then
  for index in 0 1; do
    address_var=ERC20_TOKEN_${index}_ADDRESS
    [[ $("$CAST" code "${!address_var}" --rpc-url "$RPC_URL") != 0x ]]
    [[ $("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
      'assetStatusByToken(address)(uint8)' "${!address_var}" \
      --rpc-url "$RPC_URL" | awk '{print $1}') == 1 ]]
    [[ $("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
      'canonicalTokenRegistered(address)(bool)' "${!address_var}" \
      --rpc-url "$RPC_URL") == false ]]
  done
fi

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
for _ in $(seq 1 30); do
  if docker exec "$POSTGRES_CONTAINER" createdb -U postgres actions \
      >/dev/null 2>&1; then
    break
  fi
  if docker exec "$POSTGRES_CONTAINER" psql -U postgres -d zeko_proofs \
      -Atqc "SELECT 1 FROM pg_database WHERE datname = 'actions'" \
      2>/dev/null | grep -qx 1; then
    break
  fi
  sleep 1
done
docker exec "$POSTGRES_CONTAINER" psql -U postgres -d zeko_proofs \
  -Atqc "SELECT 1 FROM pg_database WHERE datname = 'actions'" | grep -qx 1

initial_settlement="$initial_fixture_dir/settlement.json"
outer_public_key=$(jq -r '.outerAccountPublicKey' "$initial_settlement")
inner_public_key=$(jq -r '.[0][1].public_key' \
  "$FIXTURE_ROOT/bridge-genesis-ledger.json")
fee_payer_public_key=$(jq -r '.feePayerPublicKey' "$initial_settlement")
fee_payer_nonce=$(jq -r '.nonce' "$initial_settlement")
initial_action_state=$(jq -r ".$initial_action_state_field" \
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
export ETHEREUM_FINALITY_MODE=confirmations
export ETHEREUM_CONFIRMATIONS=1
export ETHEREUM_POLL_INTERVAL_SECS=1
export ETHEREUM_INDEXER_START_BLOCK=0
export BRIDGE_AUTO_PROVE_DEPOSITS=true
export BRIDGE_AUTO_PROVE_POLL_SECS=1
export API_CORS_ALLOWED_ORIGINS=http://127.0.0.1:5173
VIRTUAL_MINA_GENESIS_TIMESTAMP=$(date -u -d "@$genesis_timestamp" +%Y-%m-%dT%H:%M:%SZ)
export VIRTUAL_MINA_GENESIS_TIMESTAMP
export VIRTUAL_MINA_FORK_SLOT="$FORK_SLOT"
export VIRTUAL_MINA_OUTER_PUBLIC_KEY="$outer_public_key"
export VIRTUAL_MINA_INNER_PUBLIC_KEY="$inner_public_key"
export VIRTUAL_MINA_FEE_PAYER_PUBLIC_KEY="$fee_payer_public_key"
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

ACTIONS_DATABASE_URL="postgresql://postgres:postgres@127.0.0.1:$PG_PORT/actions"
registry_public_key=$outer_public_key
index_registry=false
if [[ $BRIDGE_ASSET == erc20 ]]; then
  registry_public_key=$(jq -er '.ethereumAssetRegistryL2' \
    "$FIXTURE_ROOT/bridge-scenario.json")
fi
(
  cd "$ZEKO_UI_ROOT"
  DATABASE_URL="$ACTIONS_DATABASE_URL" \
    "$NIX" develop -c pnpm exec moon run actions-api:db-migrate
) >/dev/null
(
  cd "$ZEKO_UI_ROOT"
  DATABASE_URL="$ACTIONS_DATABASE_URL" AUTH_TOKEN=local-actions-token \
    PORT="$ACTIONS_INDEXER_PORT" L1_ARCHIVE_URL="$API_URL/graphql" \
    L1_FINALITY=1 L2_ARCHIVE_URL="$API_URL/graphql" L2_FINALITY_TIME_H=0 \
    OUTER_PK="$outer_public_key" INNER_PK="$inner_public_key" \
    REGISTRY_PK="$registry_public_key" INDEX_REGISTRY="$index_registry" \
    INDEX_OUTER=true INDEX_INNER=true ENVIRONMENT=LOCAL \
    "$NIX" develop -c pnpm exec moon run actions-indexer:start
) >"$ACTIONS_INDEXER_LOG" 2>&1 &
ACTIONS_INDEXER_PID=$!
(
  cd "$ZEKO_UI_ROOT"
  DATABASE_URL="$ACTIONS_DATABASE_URL" ENVIRONMENT=local \
    "$NIX" develop -c pnpm exec moon run actions-api:dev -- \
      --port "$ACTIONS_API_PORT" --var ENVIRONMENT:local \
      --var "DATABASE_URL:$ACTIONS_DATABASE_URL"
) >"$ACTIONS_API_LOG" 2>&1 &
ACTIONS_API_PID=$!
for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:$ACTIONS_API_PORT/health" \
      | jq -e '.status == "healthy"' >/dev/null 2>&1; then
    break
  fi
  kill -0 "$ACTIONS_API_PID" 2>/dev/null || exit 1
  sleep 1
done
curl -fsS "http://127.0.0.1:$ACTIONS_API_PORT/health" \
  | jq -e '.status == "healthy"' >/dev/null
for _ in $(seq 1 120); do
  if curl -fsS -H 'authorization: Bearer local-actions-token' \
      "http://127.0.0.1:$ACTIONS_INDEXER_PORT/status" >/dev/null 2>&1; then
    break
  fi
  kill -0 "$ACTIONS_INDEXER_PID" 2>/dev/null || exit 1
  sleep 1
done

SDK_OUTPUT=$(mktemp)
run_eth_sdk() {
  local command=$1
  shift
  (
    cd "$ZEKO_UI_ROOT"
    GATEWAY_URL="$API_URL" RPC_URL="$RPC_URL" CHAIN_ID=31337 \
      ETHEREUM_ACCOUNT="$ADMIN_ADDRESS" E2E_OUTPUT="$SDK_OUTPUT" "$@" \
      "$NIX" develop -c pnpm exec moon run eth-bridge-sdk:e2e -- "$command"
  )
}

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

if [[ $BRIDGE_ASSET == erc20 ]]; then
  submit_settlement "$FIXTURE_ROOT/registration/settlement.json"
  registration_sequence=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
    'batchSequence()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
  [[ $registration_sequence == 1 ]]

  ASSET_BATCH_OUTPUT=$(mktemp)
  (
    cd "$ZEKO_UI_ROOT"
    "$NIX" develop -c pnpm exec moon run eth-bridge-sdk:asset-record-batch -- \
      "$FIXTURE_ROOT/bridge-scenario.json" "$ASSET_BATCH_OUTPUT"
  ) >/dev/null
  [[ $(jq 'length' "$ASSET_BATCH_OUTPUT") == 2 ]]
  batch_root=$(jq -r '.[0].root | ascii_downcase' "$ASSET_BATCH_OUTPUT")
  [[ $(jq -r '.[1].root | ascii_downcase' "$ASSET_BATCH_OUTPUT") == \
    "$batch_root" ]]
  settled_batch_root=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
    'assetRegistryRecordBatch(uint64)(bytes32,uint32,uint32,bytes32,uint32,bool)' \
    "$registration_sequence" --rpc-url "$RPC_URL" | sed -n '4p' \
    | tr '[:upper:]' '[:lower:]')
  [[ $settled_batch_root == "$batch_root" ]]

  for index in 0 1; do
    address_var=ERC20_TOKEN_${index}_ADDRESS
    proof=$(jq -r --argjson index "$index" \
      '.[ $index ].proof | "[" + join(",") + "]"' "$ASSET_BATCH_OUTPUT")
    "$CAST" send "$BRIDGE_CONTRACT_ADDRESS" \
      'activateAssetFromBatch(address,uint64,bytes32[8])' \
      "${!address_var}" "$registration_sequence" "$proof" \
      --private-key "$PRIVATE_KEY" --rpc-url "$RPC_URL" >/dev/null
    [[ $("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
      'assetStatusByToken(address)(uint8)' "${!address_var}" \
      --rpc-url "$RPC_URL" | awk '{print $1}') == 2 ]]
    [[ $("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
      'canonicalTokenRegistered(address)(bool)' "${!address_var}" \
      --rpc-url "$RPC_URL") == true ]]
  done
fi

zeko_recipient_public_key=$(jq -r '.zekoRecipientPublicKey' \
  "$FIXTURE_ROOT/bridge-scenario.json")
deposit_amount=$(jq -r '.depositAmountZeko' "$FIXTURE_ROOT/bridge-scenario.json")
deposit_nonces=()
deposit_tx_hashes=()
if [[ $BRIDGE_ASSET == native ]]; then
  ((deposit_amount % 1000000000 == 0))
  deposit_value_wei=$(bc <<<"$deposit_amount * 1000000000")
  run_eth_sdk deposit env ZEKO_RECIPIENT_PUBLIC_KEY="$zeko_recipient_public_key" \
    DEPOSIT_VALUE_WEI="$deposit_value_wei" >/dev/null
  deposit_nonces+=("$(jq -r '.nonce' "$SDK_OUTPUT")")
  deposit_tx_hashes+=("$(jq -r '.hash' "$SDK_OUTPUT")")
else
  for index in 0 1; do
    address_var=ERC20_TOKEN_${index}_ADDRESS
    run_eth_sdk token-deposit env \
      ZEKO_RECIPIENT_PUBLIC_KEY="$zeko_recipient_public_key" \
      ERC20_TOKEN_ADDRESS="${!address_var}" \
      ERC20_DEPOSIT_AMOUNT="$deposit_amount" >/dev/null
    deposit_nonces+=("$(jq -r '.nonce' "$SDK_OUTPUT")")
    deposit_tx_hashes+=("$(jq -r '.hash' "$SDK_OUTPUT")")
  done
fi
for index in "${!deposit_nonces[@]}"; do
  [[ ${deposit_nonces[index]} == $((index + 1)) ]]
done
"$CAST" rpc anvil_mine 0x2 --rpc-url "$RPC_URL" >/dev/null

for index in "${!deposit_nonces[@]}"; do
  deposit_nonce=${deposit_nonces[index]}
  bridge_job=
  for _ in $(seq 1 60); do
    deposit=$(curl -fsS "$API_URL/v1/bridge/deposits/$deposit_nonce" \
      2>/dev/null || true)
    bridge_job=$(jq -r '.bridgeJobId // empty' <<<"$deposit")
    [[ -n $bridge_job ]] && break
    sleep 1
  done
  [[ -n $bridge_job ]]
  if [[ $BRIDGE_ASSET == erc20 ]]; then
    address_var=ERC20_TOKEN_${index}_ADDRESS
    asset_var=ERC20_TOKEN_${index}_ASSET_ID
    [[ $(jq -r '.assetId | ascii_downcase' <<<"$deposit") == \
      "${!asset_var}" ]]
    [[ $(jq -r '.token | ascii_downcase' <<<"$deposit") == \
      "${!address_var}" ]]
    custody=$("$CAST" call "${!address_var}" \
      'balanceOf(address)(uint256)' "$BRIDGE_CONTRACT_ADDRESS" \
      --rpc-url "$RPC_URL" | awk '{print $1}')
    liability=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
      'escrowLiabilityByToken(address)(uint256)' "${!address_var}" \
      --rpc-url "$RPC_URL" | awk '{print $1}')
    [[ $custody == "$deposit_amount" && $liability == "$deposit_amount" ]]
  fi
  wait_job "$bridge_job"
done
"$CAST" rpc anvil_mine 0x1 --rpc-url "$RPC_URL" >/dev/null
mapfile -t deposit_auxes < <(jq -r '.depositAuxes[]' \
  "$FIXTURE_ROOT/bridge-scenario.json")
deposit_auxes_json=$(
  for aux in "${deposit_auxes[@]}"; do
    "$CAST" to-dec "$aux"
  done | jq -Rsc 'split("\n") | map(select(length > 0))'
)
for _ in $(seq 1 120); do
  indexed_witness=$(jq -n --argjson auxes "$deposit_auxes_json" \
    '{query:"query($input: OuterWitnessesFromAuxesInput!) { outerWitnessesFromAuxes(input:$input) { aux index beforeState afterState finalityStatus } }",variables:{input:{auxes:$auxes}}}' \
    | curl -fsS -H 'content-type: application/json' --data-binary @- \
        "http://127.0.0.1:$ACTIONS_API_PORT/graphql" 2>/dev/null || true)
  if [[ $(jq -r '.data.outerWitnessesFromAuxes | length // 0' \
      <<<"$indexed_witness") == "${#deposit_auxes[@]}" ]]; then
    break
  fi
  sleep 1
done
[[ $(jq -r '.data.outerWitnessesFromAuxes | length // 0' \
  <<<"$indexed_witness") == "${#deposit_auxes[@]}" ]]
[[ $("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" 'actionState()(bytes32)' \
  --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]') == \
  "$(jq -r '.outerActionStateAfterDeposit | ascii_downcase' \
    "$FIXTURE_ROOT/bridge-scenario.json")" ]]

submit_settlement "$FIXTURE_ROOT/deposit-sync/settlement.json"
for deposit_nonce in "${deposit_nonces[@]}"; do
  for _ in $(seq 1 60); do
    deposit_status=$(curl -fsS "$API_URL/v1/bridge/deposits/$deposit_nonce" \
      | jq -r '.status')
    [[ $deposit_status == synchronized ]] && break
    sleep 1
  done
  [[ $deposit_status == synchronized ]]
done

submit_settlement "$FIXTURE_ROOT/withdrawal/settlement.json"
withdrawal_recipient=$(jq -r '.withdrawalRecipient' \
  "$FIXTURE_ROOT/bridge-scenario.json")
settlement_sequence=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'batchSequence()(uint64)' --rpc-url "$RPC_URL" | awk '{print $1}')
withdrawals=()
if [[ $BRIDGE_ASSET == native ]]; then
  withdrawal=$(curl -fsS \
    "$API_URL/v1/bridge/withdrawals?recipient=$withdrawal_recipient" \
    | jq -e '.[0]')
  withdrawals+=("$(jq -c '.' <<<"$withdrawal")")
else
  for index in 0 1; do
    withdrawal=$(curl -fsS \
      "$API_URL/v1/bridge/token-withdrawals/$settlement_sequence/$index" \
      | jq -e '.')
    address_var=ERC20_TOKEN_${index}_ADDRESS
    asset_var=ERC20_TOKEN_${index}_ASSET_ID
    [[ $(jq -r '.token | ascii_downcase' <<<"$withdrawal") == \
      "${!address_var}" ]]
    [[ $(jq -r '.assetId | ascii_downcase' <<<"$withdrawal") == \
      "${!asset_var}" ]]
    withdrawals+=("$(jq -c '.' <<<"$withdrawal")")
  done
fi
claimable_slot=0
for withdrawal in "${withdrawals[@]}"; do
  candidate_slot=$(jq -r '.claimableSlot' <<<"$withdrawal")
  ((candidate_slot > claimable_slot)) && claimable_slot=$candidate_slot
done
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
claim_tx_hashes=()
withdrawal_amounts=()
for index in "${!withdrawals[@]}"; do
  if [[ $BRIDGE_ASSET == native ]]; then
    withdrawal=$(curl -fsS \
      "$API_URL/v1/bridge/withdrawals?recipient=$withdrawal_recipient" \
      | jq -e '.[0]')
  else
    withdrawal=$(curl -fsS \
      "$API_URL/v1/bridge/token-withdrawals/$settlement_sequence/$index" \
      | jq -e '.')
  fi
  [[ $(jq -r '.status' <<<"$withdrawal") == claimable ]]
  amount=$(jq -r '.amount' <<<"$withdrawal")
  withdrawal_amounts+=("$amount")

  if [[ $BRIDGE_ASSET == native ]]; then
    liability_before=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
      'nativeEscrowLiability()(uint256)' --rpc-url "$RPC_URL" | awk '{print $1}')
    run_eth_sdk claim env WITHDRAWAL_RECIPIENT="$withdrawal_recipient" >/dev/null
  else
    address_var=ERC20_TOKEN_${index}_ADDRESS
    liability_before=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
      'escrowLiabilityByToken(address)(uint256)' "${!address_var}" \
      --rpc-url "$RPC_URL" | awk '{print $1}')
    recipient_balance_before=$("$CAST" call "${!address_var}" \
      'balanceOf(address)(uint256)' "$withdrawal_recipient" \
      --rpc-url "$RPC_URL" | awk '{print $1}')
    run_eth_sdk token-claim env \
      WITHDRAWAL_SETTLEMENT_SEQUENCE="$settlement_sequence" \
      WITHDRAWAL_OFFSET="$index" >/dev/null
  fi
  claim_tx_hashes+=("$(jq -r '.hash' "$SDK_OUTPUT")")
  if [[ $BRIDGE_ASSET == native ]]; then
    liability_after=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
      'nativeEscrowLiability()(uint256)' --rpc-url "$RPC_URL" | awk '{print $1}')
    expected_release=$((amount * 1000000000))
  else
    liability_after=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
      'escrowLiabilityByToken(address)(uint256)' "${!address_var}" \
      --rpc-url "$RPC_URL" | awk '{print $1}')
    recipient_balance_after=$("$CAST" call "${!address_var}" \
      'balanceOf(address)(uint256)' "$withdrawal_recipient" \
      --rpc-url "$RPC_URL" | awk '{print $1}')
    [[ $(bc <<<"$recipient_balance_after - $recipient_balance_before") == \
      "$amount" ]]
    expected_release=$amount
  fi
  released_wei=$(bc <<<"$liability_before - $liability_after")
  [[ $released_wei == "$expected_release" ]]
done

deposit_tx_hashes_json=$(printf '%s\n' "${deposit_tx_hashes[@]}" \
  | jq -Rsc 'split("\n") | map(select(length > 0))')
claim_tx_hashes_json=$(printf '%s\n' "${claim_tx_hashes[@]}" \
  | jq -Rsc 'split("\n") | map(select(length > 0))')
tokens_json=null
asset_ids_json=null
if [[ $BRIDGE_ASSET == erc20 ]]; then
  tokens_json=$(printf '%s\n' "$ERC20_TOKEN_0_ADDRESS" "$ERC20_TOKEN_1_ADDRESS" \
    | jq -Rsc 'split("\n") | map(select(length > 0))')
  asset_ids_json=$(printf '%s\n' "$ERC20_TOKEN_0_ASSET_ID" "$ERC20_TOKEN_1_ASSET_ID" \
    | jq -Rsc 'split("\n") | map(select(length > 0))')
fi

jq -n --arg bridge "$BRIDGE_CONTRACT_ADDRESS" \
  --arg settlement "$SETTLEMENT_CONTRACT_ADDRESS" \
  --argjson depositTransactionHashes "$deposit_tx_hashes_json" \
  --argjson claimTransactionHashes "$claim_tx_hashes_json" \
  --arg bridgeAsset "$BRIDGE_ASSET" \
  --argjson tokens "$tokens_json" --argjson assetIds "$asset_ids_json" \
  --arg recipient "$withdrawal_recipient" \
  '{status:"passed",bridge:$bridge,settlement:$settlement,
    depositTransactionHashes:$depositTransactionHashes,
    claimTransactionHashes:$claimTransactionHashes,
    bridgeAsset:$bridgeAsset,
    tokens:$tokens,assetIds:$assetIds,
    actionsWitnessIndexed:true,
    withdrawalRecipient:$recipient,
    registrationSettlements:(if $bridgeAsset == "erc20" then 1 else 0 end),
    bridgeSettlements:2,
    ocamlCommits:(if $bridgeAsset == "erc20" then 3 else 2 end),
    sp1ProofsGenerated:0}'
if [[ $BRIDGE_ASSET == erc20 ]]; then
  echo "Two pending ERC20s -> registry settlement -> activation -> two deposits -> two bridge settlements -> two token claims passed."
else
  echo "ETH deposit -> bridge execute -> two real OCaml settlements -> Merkle withdrawal claim passed."
fi
echo "No SP1 proof was requested or generated."
