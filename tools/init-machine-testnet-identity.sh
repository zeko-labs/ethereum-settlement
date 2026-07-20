#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 [testnet-dir]" >&2
  exit 2
}

[[ $# -le 1 ]] || usage
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TESTNET_DIR=${1:-$ROOT/deploy/testnet}
CAST=${CAST:-$HOME/.foundry/bin/cast}
FORGE=${FORGE:-$HOME/.foundry/bin/forge}

[[ $TESTNET_DIR == /* ]] || TESTNET_DIR="$ROOT/$TESTNET_DIR"
mkdir -p "$TESTNET_DIR"
TESTNET_DIR=$(realpath "$TESTNET_DIR")
[[ -x $CAST && -x $FORGE ]] || {
  echo "Missing cast or forge" >&2
  exit 1
}
for path in "$TESTNET_DIR/.env" "$TESTNET_DIR/secrets/admin-private-key" \
  "$TESTNET_DIR/secrets/upgrader-private-key"; do
  [[ ! -e $path ]] || {
    echo "Refusing to overwrite existing testnet identity: $path" >&2
    exit 1
  }
done

new_evm_key() {
  local key
  while true; do
    key="0x$(openssl rand -hex 32)"
    if "$CAST" wallet address --private-key "$key" >/dev/null 2>&1; then
      printf '%s\n' "$key"
      return
    fi
  done
}

admin_private_key=$(new_evm_key)
upgrader_private_key=$(new_evm_key)
admin_address=$("$CAST" wallet address --private-key "$admin_private_key")
upgrader_address=$("$CAST" wallet address --private-key "$upgrader_private_key")

prediction=$(
  cd "$ROOT/contracts"
  ADMIN_ADDRESS="$admin_address" "$FORGE" script \
    script/PredictPocDeployment.s.sol:PredictPocDeployment 2>&1
)
bridge_address=$(awk '/BRIDGE_CONTRACT_ADDRESS/ { value=$NF } END { print value }' \
  <<<"$prediction")
[[ $bridge_address =~ ^0x[0-9a-fA-F]{40}$ ]] || {
  echo "Could not predict bridge proxy address" >&2
  exit 1
}

"$ROOT/tools/init-testnet-secrets.sh" "$bridge_address" "$TESTNET_DIR" >/dev/null

umask 077
printf '%s\n' "$admin_private_key" >"$TESTNET_DIR/secrets/admin-private-key"
printf '%s\n' "$upgrader_private_key" >"$TESTNET_DIR/secrets/upgrader-private-key"
chmod 0600 "$TESTNET_DIR/secrets/admin-private-key" \
  "$TESTNET_DIR/secrets/upgrader-private-key"

gateway_address=$("$CAST" wallet address --private-key \
  "$(tr -d '\r\n' <"$TESTNET_DIR/secrets/settlement-private-key")")
network_requester_address=$("$CAST" wallet address --private-key \
  "$(tr -d '\r\n' <"$TESTNET_DIR/secrets/network-private-key")")

awk -v admin="$admin_address" -v upgrader="$upgrader_address" \
  -v gateway="$gateway_address" '
    /^ADMIN_ADDRESS=/ { print "ADMIN_ADDRESS=" admin; next }
    /^UPGRADER_ADDRESS=/ { print "UPGRADER_ADDRESS=" upgrader; next }
    /^GATEWAY_PROVER_ADDRESS=/ {
      print "GATEWAY_PROVER_ADDRESS=" gateway; next
    }
    { print }
  ' "$TESTNET_DIR/.env" >"$TESTNET_DIR/.env.tmp"
mv "$TESTNET_DIR/.env.tmp" "$TESTNET_DIR/.env"
chmod 0600 "$TESTNET_DIR/.env"

{
  printf 'ADMIN_ADDRESS=%q\n' "$admin_address"
  printf 'UPGRADER_ADDRESS=%q\n' "$upgrader_address"
  printf 'GATEWAY_PROVER_ADDRESS=%q\n' "$gateway_address"
  printf 'BRIDGE_CONTRACT_ADDRESS=%q\n' "$bridge_address"
} >"$TESTNET_DIR/secrets/deployment-roles.env"
chmod 0600 "$TESTNET_DIR/secrets/deployment-roles.env"

jq -n --arg admin "$admin_address" --arg upgrader "$upgrader_address" \
  --arg gatewayProver "$gateway_address" --arg bridge "$bridge_address" \
  --arg networkRequester "$network_requester_address" \
  --arg minaSigningNetworkId testnet \
  '{schemaVersion:1,admin:$admin,upgrader:$upgrader,
    gatewayProver:$gatewayProver,predictedBridge:$bridge,
    networkRequester:$networkRequester,
    minaSigningNetworkId:$minaSigningNetworkId}' \
  >"$TESTNET_DIR/config/identity.json"

jq -n --arg directory "$TESTNET_DIR" \
  --slurpfile identity "$TESTNET_DIR/config/identity.json" \
  '{directory:$directory,identity:$identity[0],
    next:"fund admin, gateway prover, and network requester; then prepare and deploy the PoC"}'
