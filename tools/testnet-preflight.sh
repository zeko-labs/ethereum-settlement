#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DEPLOY_DIR=${1:-$ROOT/deploy/testnet}
CAST=${CAST:-$HOME/.foundry/bin/cast}

if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose)
else
  echo "Docker Compose v2 is required" >&2
  exit 1
fi

[[ $DEPLOY_DIR == /* ]] || DEPLOY_DIR="$ROOT/$DEPLOY_DIR"
DEPLOY_DIR=$(realpath "$DEPLOY_DIR")
for file in .env gateway.env compose.yaml config/circuits.json \
  config/bridge-genesis-ledger.json config/virtual-mina-accounts.json \
  config/bridge-scenario.json config/identity.json artifacts/manifest.json \
  artifacts/images.json; do
  [[ -f "$DEPLOY_DIR/$file" ]] || {
    echo "Missing testnet input: $DEPLOY_DIR/$file" >&2
    exit 1
  }
done
[[ -x "$CAST" ]] || {
  echo "Missing cast: $CAST" >&2
  exit 1
}

set -a
source "$DEPLOY_DIR/.env"
source "$DEPLOY_DIR/gateway.env"
set +a

for name in GATEWAY_IMAGE ZEKO_IMAGE ZEKO_DA_IMAGE POSTGRES_IMAGE RABBITMQ_IMAGE; do
  value=${!name:-}
  [[ $value =~ @sha256:[0-9a-fA-F]{64}$ ]] || {
    echo "$name must be an immutable repo@sha256 digest" >&2
    exit 1
  }
done
images_manifest="$DEPLOY_DIR/artifacts/images.json"
[[ $(jq -r '.gateway' "$images_manifest") == "$GATEWAY_IMAGE" ]]
[[ $(jq -r '.zeko' "$images_manifest") == "$ZEKO_IMAGE" ]]
[[ $(jq -r '.zekoDa' "$images_manifest") == "$ZEKO_DA_IMAGE" ]]
[[ $(jq -r '.postgres' "$images_manifest") == "$POSTGRES_IMAGE" ]]
[[ $(jq -r '.rabbitmq' "$images_manifest") == "$RABBITMQ_IMAGE" ]]
[[ ${MINA_SIGNING_NETWORK_ID:-} == testnet ]] || {
  echo "MINA_SIGNING_NETWORK_ID must be exactly testnet for the Auro PoC" >&2
  exit 1
}
[[ ${ZEKO_UI_COMMIT:-} =~ ^[0-9a-f]{40}$ && -d ${ZEKO_UI_ROOT:-}/.git ]] || {
  echo "ZEKO_UI_ROOT and an exact ZEKO_UI_COMMIT are required" >&2
  exit 1
}
[[ $(git -C "$ZEKO_UI_ROOT" rev-parse HEAD) == "$ZEKO_UI_COMMIT" && \
   -z $(git -C "$ZEKO_UI_ROOT" status --porcelain) ]] || {
  echo "Zeko UI checkout must be clean and at ZEKO_UI_COMMIT" >&2
  exit 1
}
[[ ${API_REQUIRE_PROOF_APPROVAL,,} == true ]]
[[ ${API_EXECUTE_ONLY,,} == false && ${API_LOCAL_MOCK_SUBMIT,,} == false ]]
[[ $ETHEREUM_CONFIRMATIONS == 12 ]]
[[ -n ${PROVER_GAS_LIMIT:-} && -n ${PROVER_MAX_PRICE_PER_PGU:-} ]] || {
  echo "Set deployment-wide PROVER_GAS_LIMIT and PROVER_MAX_PRICE_PER_PGU hard caps" >&2
  exit 1
}

IFS=',' read -r -a da_keys <<<"${DA_PUBLIC_KEYS:-}"
[[ ${#da_keys[@]} -eq 3 ]] || {
  echo "DA_PUBLIC_KEYS must contain exactly three keys" >&2
  exit 1
}
[[ ${da_keys[0]} != "${da_keys[1]}" && ${da_keys[0]} != "${da_keys[2]}" && \
   ${da_keys[1]} != "${da_keys[2]}" ]] || {
  echo "DA public keys must be distinct" >&2
  exit 1
}
[[ $(jq -r '.commitValidityPeriod' \
  "$DEPLOY_DIR/config/bridge-scenario.json") == 2400 ]] || {
  echo "Bridge scenario must bind the 2400-slot testnet commit validity period" >&2
  exit 1
}
[[ $(jq -r '.sequencerPublicKey' \
  "$DEPLOY_DIR/config/bridge-scenario.json") == "$SEQUENCER_PUBLIC_KEY" ]] || {
  echo "SEQUENCER_PUBLIC_KEY differs from the OCaml bridge scenario" >&2
  exit 1
}
[[ -n ${BRIDGE_RECIPIENT_PUBLIC_KEY:-} && \
   $(jq -r '.zekoRecipientPublicKey' \
     "$DEPLOY_DIR/config/bridge-scenario.json") == \
     "$BRIDGE_RECIPIENT_PUBLIC_KEY" ]] || {
  echo "BRIDGE_RECIPIENT_PUBLIC_KEY differs from the OCaml bridge scenario" >&2
  exit 1
}

for secret in proof-api-key actions-indexer-token network-private-key \
  admin-private-key upgrader-private-key \
  deployment-roles.env \
  settlement-private-key \
  bridge-private-key withdraw-private-key postgres-gateway-password \
  postgres-sequencer-password rabbitmq-password sequencer-private-key \
  sequencer-signer-token da1-private-key da1-signer-token da2-private-key \
  da2-signer-token da3-private-key da3-signer-token \
  bridge-recipient-private-key signer-tls.crt signer-tls.key; do
  path="$DEPLOY_DIR/secrets/$secret"
  [[ -s "$path" ]] || {
    echo "Missing or empty secret: $path" >&2
    exit 1
  }
  mode=$(stat -c '%a' "$path")
  if [[ $secret != signer-tls.crt && $mode != 600 && $mode != 400 ]]; then
    echo "Secret $path must have mode 0600 or 0400, got $mode" >&2
    exit 1
  fi
done

settlement_sender=$(
  "$CAST" wallet address --private-key \
    "$(tr -d '\r\n' <"$DEPLOY_DIR/secrets/settlement-private-key")"
)
admin_sender=$(
  "$CAST" wallet address --private-key \
    "$(tr -d '\r\n' <"$DEPLOY_DIR/secrets/admin-private-key")"
)
upgrader_sender=$(
  "$CAST" wallet address --private-key \
    "$(tr -d '\r\n' <"$DEPLOY_DIR/secrets/upgrader-private-key")"
)
bridge_sender=$(
  "$CAST" wallet address --private-key \
    "$(tr -d '\r\n' <"$DEPLOY_DIR/secrets/bridge-private-key")"
)
withdraw_sender=$(
  "$CAST" wallet address --private-key \
    "$(tr -d '\r\n' <"$DEPLOY_DIR/secrets/withdraw-private-key")"
)
[[ ${settlement_sender,,} == "${bridge_sender,,}" && \
   ${settlement_sender,,} == "${withdraw_sender,,}" ]] || {
  echo "The PoC deployment grants one gateway prover address; all three submitter keys must match" >&2
  exit 1
}
[[ ${admin_sender,,} == "${ADMIN_ADDRESS,,}" && \
   ${upgrader_sender,,} == "${UPGRADER_ADDRESS,,}" && \
   ${settlement_sender,,} == "${GATEWAY_PROVER_ADDRESS,,}" ]] || {
  echo "Retained EVM keys do not match their configured role addresses" >&2
  exit 1
}
[[ ${ADMIN_ADDRESS,,} != "${UPGRADER_ADDRESS,,}" && \
   ${ADMIN_ADDRESS,,} != "${GATEWAY_PROVER_ADDRESS,,}" && \
   ${UPGRADER_ADDRESS,,} != "${GATEWAY_PROVER_ADDRESS,,}" ]] || {
  echo "Admin, upgrader, and gateway prover identities must be distinct" >&2
  exit 1
}

chain_id=$("$CAST" chain-id --rpc-url "$RPC_URL")
[[ $chain_id == 11155111 ]] || {
  echo "RPC_URL is chain $chain_id, expected Sepolia 11155111" >&2
  exit 1
}
manifest="$DEPLOY_DIR/artifacts/manifest.json"
[[ $(jq -r '.chainId' "$manifest") == 11155111 ]]
[[ $(jq -r '.schemaVersion' "$manifest") == 2 ]]
[[ $(jq -r '.dataAvailability' "$manifest") == multisig ]]
[[ $(jq -r '.minaSigningNetworkId' "$manifest") == testnet ]]
[[ $(jq -r '.admin | ascii_downcase' "$manifest") == "${ADMIN_ADDRESS,,}" ]]
[[ $(jq -r '.upgrader | ascii_downcase' "$manifest") == \
  "${UPGRADER_ADDRESS,,}" ]]
[[ $(jq -r '.gatewayProver | ascii_downcase' "$manifest") == \
  "${GATEWAY_PROVER_ADDRESS,,}" ]]
[[ $(jq -r '.settlement | ascii_downcase' "$manifest") == \
   "${SETTLEMENT_CONTRACT_ADDRESS,,}" ]]
[[ $(jq -r '.bridge | ascii_downcase' "$manifest") == \
   "${BRIDGE_CONTRACT_ADDRESS,,}" ]]
[[ 0x$(jq -r '.settlementVkSha256' "$images_manifest") == \
  $(jq -r '.settlementVkHash | ascii_downcase' "$manifest") ]]
holder=$(jq -r '.ocamlEthereumHolderX | ascii_downcase' "$manifest")
bridge_hex=${BRIDGE_CONTRACT_ADDRESS#0x}
bridge_hex=${bridge_hex,,}
[[ ${holder: -40} == "$bridge_hex" ]]

for address in "$SETTLEMENT_CONTRACT_ADDRESS" "$BRIDGE_CONTRACT_ADDRESS"; do
  code=$("$CAST" code "$address" --rpc-url "$RPC_URL")
  [[ $code != 0x && ${#code} -gt 4 ]] || {
    echo "No contract code at $address" >&2
    exit 1
  }
done
official_verifier=$(jq -er '.V6_1_0_SP1_VERIFIER_GROTH16' \
  "$ROOT/contracts/lib/sp1-contracts/contracts/deployments/11155111.json")
manifest_verifier=$(jq -r '.sp1Verifier | ascii_downcase' "$manifest")
[[ $manifest_verifier == "${official_verifier,,}" && \
   $manifest_verifier != $(jq -r '.localSp1Verifier | ascii_downcase' "$manifest") ]] || {
  echo "Manifest does not bind the official SP1 v6.1 Groth16 verifier" >&2
  exit 1
}
verifier_code=$("$CAST" code "$official_verifier" --rpc-url "$RPC_URL")
[[ $verifier_code != 0x && ${#verifier_code} -gt 4 ]] || {
  echo "Official SP1 verifier is missing on Sepolia" >&2
  exit 1
}
settlement_verifier=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'verifier()(address)' --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]')
bridge_verifier=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'bridgeVerifier()(address)' --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]')
withdraw_verifier=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'withdrawVerifier()(address)' --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]')
[[ $settlement_verifier == "${official_verifier,,}" && \
   $bridge_verifier == "${official_verifier,,}" && \
   $withdraw_verifier == "${official_verifier,,}" ]] || {
  echo "Settlement or bridge is wired to the wrong SP1 verifier" >&2
  exit 1
}
settlement_role=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'PROVER_ROLE()(bytes32)' --rpc-url "$RPC_URL")
bridge_role=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'PROVER_ROLE()(bytes32)' --rpc-url "$RPC_URL")
[[ $("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'hasRole(bytes32,address)(bool)' "$settlement_role" "$settlement_sender" \
  --rpc-url "$RPC_URL") == true ]] || {
  echo "Gateway submitter lacks settlement PROVER_ROLE" >&2
  exit 1
}
[[ $("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'hasRole(bytes32,address)(bool)' "$bridge_role" "$settlement_sender" \
  --rpc-url "$RPC_URL") == true ]] || {
  echo "Gateway submitter lacks bridge PROVER_ROLE" >&2
  exit 1
}

assert_role_shape() {
  local contract=$1
  local admin_role prover_role upgrader_role
  admin_role=$("$CAST" call "$contract" 'ADMIN_ROLE()(bytes32)' --rpc-url "$RPC_URL")
  prover_role=$("$CAST" call "$contract" 'PROVER_ROLE()(bytes32)' --rpc-url "$RPC_URL")
  upgrader_role=$("$CAST" call "$contract" 'UPGRADER_ROLE()(bytes32)' --rpc-url "$RPC_URL")
  local default_admin=0x0000000000000000000000000000000000000000000000000000000000000000

  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$default_admin" "$ADMIN_ADDRESS" --rpc-url "$RPC_URL") == true ]]
  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$admin_role" "$ADMIN_ADDRESS" --rpc-url "$RPC_URL") == true ]]
  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$prover_role" "$ADMIN_ADDRESS" --rpc-url "$RPC_URL") == false ]]
  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$upgrader_role" "$ADMIN_ADDRESS" --rpc-url "$RPC_URL") == false ]]

  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$prover_role" "$GATEWAY_PROVER_ADDRESS" --rpc-url "$RPC_URL") == true ]]
  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$admin_role" "$GATEWAY_PROVER_ADDRESS" --rpc-url "$RPC_URL") == false ]]
  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$upgrader_role" "$GATEWAY_PROVER_ADDRESS" --rpc-url "$RPC_URL") == false ]]

  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$upgrader_role" "$UPGRADER_ADDRESS" --rpc-url "$RPC_URL") == true ]]
  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$admin_role" "$UPGRADER_ADDRESS" --rpc-url "$RPC_URL") == false ]]
  [[ $("$CAST" call "$contract" 'hasRole(bytes32,address)(bool)' \
    "$prover_role" "$UPGRADER_ADDRESS" --rpc-url "$RPC_URL") == false ]]
}

assert_role_shape "$SETTLEMENT_CONTRACT_ADDRESS"
assert_role_shape "$BRIDGE_CONTRACT_ADDRESS"

[[ $(jq -r '.sourceRevisions.ethereumSettlement' "$manifest") == \
  $(git -C "$ROOT" rev-parse HEAD) ]]
[[ $(jq -r '.sourceRevisions.zeko' "$manifest") == \
  $(git -C /root/zeko rev-parse HEAD) ]]
[[ $(jq -r '.sourceRevisions.zekoUi' "$manifest") == "$ZEKO_UI_COMMIT" ]]
settlement_vkey=$("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'programVKey()(bytes32)' --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]')
bridge_vkey=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'bridgeProgramVKey()(bytes32)' --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]')
withdraw_vkey=$("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'withdrawProgramVKey()(bytes32)' --rpc-url "$RPC_URL" | tr '[:upper:]' '[:lower:]')
[[ $settlement_vkey == $(jq -r '.settlementProgramVkey | ascii_downcase' "$manifest") ]]
[[ $bridge_vkey == $(jq -r '.bridgeProgramVkey | ascii_downcase' "$manifest") ]]
[[ $withdraw_vkey == $(jq -r '.withdrawProgramVkey | ascii_downcase' "$manifest") ]]
[[ $("$CAST" call "$SETTLEMENT_CONTRACT_ADDRESS" \
  'slotDuration()(uint32)' --rpc-url "$RPC_URL" | awk '{print $1}') == 12 ]]
[[ $("$CAST" call "$BRIDGE_CONTRACT_ADDRESS" \
  'withdrawalDelaySlots()(uint32)' --rpc-url "$RPC_URL" | awk '{print $1}') == 5 ]]

manifest_da=$(jq -r '.daPublicKeys? // empty | join(",")' \
  "$DEPLOY_DIR/config/bridge-scenario.json" 2>/dev/null || true)
if [[ -n $manifest_da && $manifest_da != "$DA_PUBLIC_KEYS" ]]; then
  echo "DA_PUBLIC_KEYS differ from the OCaml bridge scenario" >&2
  exit 1
fi

(cd "$DEPLOY_DIR" && "${COMPOSE[@]}" --env-file .env -f compose.yaml config --quiet)

jq -n --arg rpc "$RPC_URL" --arg settlement "$SETTLEMENT_CONTRACT_ADDRESS" \
  --arg bridge "$BRIDGE_CONTRACT_ADDRESS" \
  '{status:"ready",chainId:11155111,rpc:$rpc,settlement:$settlement,
    bridge:$bridge,daQuorum:"2-of-3",proofApprovalRequired:true,
    confirmations:12}'
