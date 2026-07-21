#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source "$ROOT/tools/lib/workspace.sh"
zeko_resolve_companion_repo "$ROOT" ZEKO_ROOT zeko src/app/zeko
zeko_resolve_companion_repo "$ROOT" ZEKO_UI_ROOT zeko-ui packages/eth-bridge-sdk

NIX=${NIX:-$HOME/.nix-profile/bin/nix}
FORGE=${FORGE:-$HOME/.foundry/bin/forge}
CAST=${CAST:-$HOME/.foundry/bin/cast}
ADMIN_ADDRESS=${ADMIN_ADDRESS:-0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266}
FIXTURE_ROOT=${BRIDGE_FIXTURE_ROOT:-$ROOT/build/poc/bridge-erc20-fixtures}
LIVE_DIR=${BRIDGE_LIVE_STATE_DIR:-$ROOT/build/poc/bridge-erc20-live}
ERC20_DEPOSIT_CAP=${ERC20_DEPOSIT_CAP:-100000000000}
ERC20_DEPOSIT_AMOUNT=${ERC20_DEPOSIT_AMOUNT:-10000000000}

for executable in "$NIX" "$FORGE" "$CAST"; do
  [[ -x $executable ]] || {
    echo "Missing executable: $executable" >&2
    exit 1
  }
done
for repository in "$ROOT" "$ZEKO_ROOT" "$ZEKO_UI_ROOT"; do
  zeko_is_clean_checkout "$repository" || {
    echo "ERC20 roundtrip requires a clean exact source revision: $repository" >&2
    exit 1
  }
done

TEMP_DIR=$(mktemp -d)
cleanup() {
  local status=$?
  trap - EXIT INT TERM
  rm -rf "$TEMP_DIR"
  exit "$status"
}
trap cleanup EXIT INT TERM

echo "Building the live Zeko bridge harness..."
(
  cd "$ZEKO_ROOT"
  "$NIX" develop "git+file://$ZEKO_ROOT?submodules=1" \
    --accept-flake-config --max-jobs auto -c \
    dune build ./src/app/zeko/sequencer ./src/app/zeko/da_layer \
      ./src/app/zeko/signer
)

KEYGEN=$ZEKO_ROOT/_build/default/src/app/zeko/sequencer/cli.exe
generate_even_private_key() {
  ZEKO_CIRCUITS_CONFIG=test "$KEYGEN" generate-even-key \
    | awk -F': ' '/Private key:/ {print $2}'
}

ERC20_TOKEN_OWNER_PRIVATE_KEY=$(generate_even_private_key)
ERC20_TOKEN_VAULT_PRIVATE_KEY=$(generate_even_private_key)
ERC20_ADMIN_CONTRACT_PRIVATE_KEY=$(generate_even_private_key)
ERC20_ADMIN_AUTHORITY_PRIVATE_KEY=$(generate_even_private_key)
ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY=$(generate_even_private_key)
export ERC20_TOKEN_OWNER_PRIVATE_KEY ERC20_TOKEN_VAULT_PRIVATE_KEY
export ERC20_ADMIN_CONTRACT_PRIVATE_KEY ERC20_ADMIN_AUTHORITY_PRIVATE_KEY
export ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY

IDENTITY_FILE=$TEMP_DIR/erc20-identity.json
(
  cd "$ZEKO_UI_ROOT"
  ERC20_IDENTITY_OUTPUT="$IDENTITY_FILE" \
    "$NIX" develop -c pnpm exec moon run eth-bridge-sdk:erc20-identity
) >/dev/null

prediction=$(
  cd "$ROOT/contracts"
  ADMIN_ADDRESS="$ADMIN_ADDRESS" \
    "$FORGE" script script/PredictPocDeployment.s.sol:PredictPocDeployment 2>&1
)
read_prediction() {
  local name=$1
  awk -v name="$name" '$0 ~ name { value=$NF } END { if (value == "") exit 1; print value }' \
    <<<"$prediction"
}
BRIDGE_CONTRACT_ADDRESS=$(read_prediction BRIDGE_CONTRACT_ADDRESS)
ERC20_TOKEN_ADDRESS=$(read_prediction ERC20_TOKEN_ADDRESS)
export BRIDGE_CONTRACT_ADDRESS ERC20_TOKEN_ADDRESS ERC20_DEPOSIT_CAP ERC20_DEPOSIT_AMOUNT

ERC20_ZEKO_TOKEN_OWNER=$(jq -er '.tokenOwnerPacked' "$IDENTITY_FILE")
ERC20_ZEKO_TOKEN_ID=$(jq -er '.tokenIdHex' "$IDENTITY_FILE")
ERC20_TOKEN_OWNER_L2=$(jq -er '.tokenOwner' "$IDENTITY_FILE")
ERC20_TOKEN_VAULT_L2=$(jq -er '.tokenVault' "$IDENTITY_FILE")
export ERC20_ZEKO_TOKEN_OWNER ERC20_ZEKO_TOKEN_ID

asset_domain=$("$CAST" keccak 'ZEKO_ERC20_ASSET_V1')
asset_preimage=$("$CAST" abi-encode \
  'f(bytes32,uint256,address,address,bytes32,bytes32,uint8)' \
  "$asset_domain" 31337 "$BRIDGE_CONTRACT_ADDRESS" "$ERC20_TOKEN_ADDRESS" \
  "$ERC20_ZEKO_TOKEN_OWNER" "$ERC20_ZEKO_TOKEN_ID" 9)
ERC20_ASSET_ID=$("$CAST" keccak "$asset_preimage" | tr '[:upper:]' '[:lower:]')
export ERC20_ASSET_ID

ZEKO_CIRCUITS_CONFIG=$TEMP_DIR/circuits-config.json
ZEKO_DEPLOY_CONFIG=$TEMP_DIR/deploy-config.json
(
  cd "$ZEKO_ROOT"
  ZEKO_CIRCUITS_CONFIG=test \
    "$NIX" develop "git+file://$ZEKO_ROOT?submodules=1" \
      --accept-flake-config -c "$KEYGEN" generate-circuits-config \
      --circuits-config-output "$ZEKO_CIRCUITS_CONFIG" \
      --deploy-config-output "$ZEKO_DEPLOY_CONFIG" \
      --ethereum-bridge-address "$BRIDGE_CONTRACT_ADDRESS" \
      --ethereum-token-asset-id "$ERC20_ASSET_ID" \
      --ethereum-token-address "$ERC20_TOKEN_ADDRESS" \
      --ethereum-token-owner-l2 "$ERC20_TOKEN_OWNER_L2" \
      --ethereum-token-vault-l2 "$ERC20_TOKEN_VAULT_L2"
)
export ZEKO_CIRCUITS_CONFIG ZEKO_DEPLOY_CONFIG

LIVE_ENV=$TEMP_DIR/live-deployment.env
{
  printf 'BRIDGE_CONTRACT_ADDRESS=%s\n' "$BRIDGE_CONTRACT_ADDRESS"
  printf 'MINA_SIGNING_NETWORK_ID=testnet\n'
  printf 'ZEKO_CIRCUITS_CONFIG=%s\n' "$ZEKO_CIRCUITS_CONFIG"
  printf 'ZEKO_DEPLOY_CONFIG=%s\n' "$ZEKO_DEPLOY_CONFIG"
} >"$LIVE_ENV"

echo "Running the standard-token deposit/withdrawal through the live sequencer..."
BRIDGE_ASSET=erc20 POC_ENV_FILE="$LIVE_ENV" \
  BRIDGE_LIVE_FIXTURE_ROOT="$FIXTURE_ROOT" \
  BRIDGE_LIVE_STATE_DIR="$LIVE_DIR" \
  "$ROOT/tools/run-live-sequencer-bridge-e2e.sh"

scenario=$FIXTURE_ROOT/bridge-scenario.json
[[ $(jq -er '.ethereumTokenAddress | ascii_downcase' "$scenario") == \
  "${ERC20_TOKEN_ADDRESS,,}" ]]
[[ $(jq -er '.ethereumTokenAssetId | ascii_downcase' "$scenario") == \
  "$ERC20_ASSET_ID" ]]
[[ $(jq -er '.ethereumTokenOwnerPacked | ascii_downcase' "$scenario") == \
  "${ERC20_ZEKO_TOKEN_OWNER,,}" ]]
[[ $(jq -er '.ethereumTokenIdL2 | ascii_downcase' "$scenario") == \
  "${ERC20_ZEKO_TOKEN_ID,,}" ]]

echo "Submitting the exact asset-bound fixtures through Anvil custody..."
BRIDGE_ASSET=erc20 BRIDGE_FIXTURE_ROOT="$FIXTURE_ROOT" \
  "$ROOT/tools/run-local-bridge-roundtrip.sh"

jq -n --arg fixtures "$FIXTURE_ROOT" \
  --arg bridge "$BRIDGE_CONTRACT_ADDRESS" \
  --arg token "$ERC20_TOKEN_ADDRESS" --arg assetId "$ERC20_ASSET_ID" \
  '{status:"passed",bridgeAsset:"erc20",fixtures:$fixtures,
    bridge:$bridge,token:$token,assetId:$assetId,
    liveStandardTokenRoundtrip:true,anvilCustodyRoundtrip:true,
    ocamlSettlements:2,sp1ProofsGenerated:0}'
echo "Full local ERC20 bridge roundtrip passed without generating an SP1 proof."
