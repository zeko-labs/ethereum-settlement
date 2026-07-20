#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 <poc-output-dir> [testnet-dir]" >&2
  exit 2
}

[[ $# -ge 1 && $# -le 2 ]] || usage
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
POC_OUTPUT=$(realpath "$1")
TESTNET_DIR=${2:-$ROOT/deploy/testnet}
FORGE=${FORGE:-$HOME/.foundry/bin/forge}
CAST=${CAST:-$HOME/.foundry/bin/cast}

[[ $TESTNET_DIR == /* ]] || TESTNET_DIR="$ROOT/$TESTNET_DIR"
TESTNET_DIR=$(realpath "$TESTNET_DIR")
for file in "$POC_OUTPUT/deployment.env" \
  "$TESTNET_DIR/secrets/admin-private-key"; do
  [[ -s $file ]] || {
    echo "Missing deployment input: $file" >&2
    exit 1
  }
done
[[ -x $FORGE && -x $CAST ]] || {
  echo "Missing forge or cast" >&2
  exit 1
}
[[ ${CONFIRM_SEPOLIA_DEPLOY:-} == yes ]] || {
  echo "Set CONFIRM_SEPOLIA_DEPLOY=yes to authorize the Sepolia deployment" >&2
  exit 1
}

set -a
source "$POC_OUTPUT/deployment.env"
set +a
PRIVATE_KEY=$(tr -d '\r\n' <"$TESTNET_DIR/secrets/admin-private-key")
export PRIVATE_KEY LOCAL_MOCK_VERIFIER=false
[[ $("$CAST" chain-id --rpc-url "$RPC_URL") == 11155111 ]] || {
  echo "RPC_URL is not Sepolia" >&2
  exit 1
}
sender=$("$CAST" wallet address --private-key "$PRIVATE_KEY")
[[ ${sender,,} == "${ADMIN_ADDRESS,,}" ]] || {
  echo "Admin private key does not match prepared ADMIN_ADDRESS" >&2
  exit 1
}
balance=$("$CAST" balance "$ADMIN_ADDRESS" --rpc-url "$RPC_URL")
((balance > 0)) || {
  echo "Admin address has no Sepolia ETH" >&2
  exit 1
}

start_block=$("$CAST" block-number --rpc-url "$RPC_URL")
mkdir -p "$POC_OUTPUT/logs"
(
  cd "$ROOT/contracts"
  "$FORGE" script script/DeployPoc.s.sol:DeployPoc \
    --rpc-url "$RPC_URL" --broadcast --slow -vvvv
) 2>&1 | tee "$POC_OUTPUT/logs/deploy-sepolia.log"

for address in "$SETTLEMENT_CONTRACT_ADDRESS" "$BRIDGE_CONTRACT_ADDRESS"; do
  code=$("$CAST" code "$address" --rpc-url "$RPC_URL")
  [[ $code != 0x && ${#code} -gt 4 ]] || {
    echo "Deployment completed without code at $address" >&2
    exit 1
  }
done

if grep -q '^ETHEREUM_INDEXER_START_BLOCK=' "$POC_OUTPUT/deployment.env"; then
  sed -i "s/^ETHEREUM_INDEXER_START_BLOCK=.*/ETHEREUM_INDEXER_START_BLOCK=$start_block/" \
    "$POC_OUTPUT/deployment.env"
else
  printf 'ETHEREUM_INDEXER_START_BLOCK=%s\n' "$start_block" \
    >>"$POC_OUTPUT/deployment.env"
fi

jq -n --argjson startBlock "$start_block" \
  --arg settlement "$SETTLEMENT_CONTRACT_ADDRESS" \
  --arg bridge "$BRIDGE_CONTRACT_ADDRESS" \
  --arg admin "$ADMIN_ADDRESS" --arg upgrader "$UPGRADER_ADDRESS" \
  --arg gatewayProver "$GATEWAY_PROVER_ADDRESS" \
  '{status:"deployed",chainId:11155111,startBlock:$startBlock,
    settlement:$settlement,bridge:$bridge,admin:$admin,
    upgrader:$upgrader,gatewayProver:$gatewayProver}' \
  >"$POC_OUTPUT/deployment-result.json"
cat "$POC_OUTPUT/deployment-result.json"
