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
ERC20_TOKEN_0_DEPOSIT_CAP=${ERC20_TOKEN_0_DEPOSIT_CAP:-100000000000}
ERC20_TOKEN_1_DEPOSIT_CAP=${ERC20_TOKEN_1_DEPOSIT_CAP:-200000000000}
ERC20_TOKEN_0_DEPOSIT_AMOUNT=${ERC20_TOKEN_0_DEPOSIT_AMOUNT:-10000000000}
ERC20_TOKEN_1_DEPOSIT_AMOUNT=${ERC20_TOKEN_1_DEPOSIT_AMOUNT:-10000000000}
ERC20_MFT_STANDARD_VK_ID_DECIMAL=${ERC20_MFT_STANDARD_VK_ID_DECIMAL:-9001}
ERC20_UNIVERSAL_BRIDGE_VK_ID_DECIMAL=${ERC20_UNIVERSAL_BRIDGE_VK_ID_DECIMAL:-9002}

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
generate_even_key() {
  ZEKO_CIRCUITS_CONFIG=test "$KEYGEN" generate-even-key
}
key_field() {
  local label=$1
  awk -F': ' -v label="$label" '$1 == label {print $2}'
}

registry_key=$(generate_even_key)
vault_key=$(generate_even_key)
recipient_key=$(generate_even_key)
ERC20_REGISTRY_L2=$(key_field 'Public key' <<<"$registry_key")
ERC20_SHARED_VAULT_L2=$(key_field 'Public key' <<<"$vault_key")
ERC20_SHARED_VAULT_PRIVATE_KEY=$(key_field 'Private key' <<<"$vault_key")
ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY=$(key_field 'Private key' <<<"$recipient_key")

for index in 0 1; do
  owner_key=$(generate_even_key)
  admin_contract_key=$(generate_even_key)
  admin_authority_key=$(generate_even_key)
  printf -v "ERC20_TOKEN_${index}_OWNER_PRIVATE_KEY" '%s' \
    "$(key_field 'Private key' <<<"$owner_key")"
  printf -v "ERC20_TOKEN_${index}_OWNER_L2" '%s' \
    "$(key_field 'Public key' <<<"$owner_key")"
  printf -v "ERC20_TOKEN_${index}_ADMIN_CONTRACT_PRIVATE_KEY" '%s' \
    "$(key_field 'Private key' <<<"$admin_contract_key")"
  printf -v "ERC20_TOKEN_${index}_ADMIN_AUTHORITY_PRIVATE_KEY" '%s' \
    "$(key_field 'Private key' <<<"$admin_authority_key")"
done

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
ERC20_TOKEN_0_ADDRESS=$(read_prediction ERC20_TOKEN_0_ADDRESS)
ERC20_TOKEN_1_ADDRESS=$(read_prediction ERC20_TOKEN_1_ADDRESS)

for index in 0 1; do
  identity_file=$TEMP_DIR/erc20-identity-$index.json
  owner_private_var=ERC20_TOKEN_${index}_OWNER_PRIVATE_KEY
  admin_contract_var=ERC20_TOKEN_${index}_ADMIN_CONTRACT_PRIVATE_KEY
  admin_authority_var=ERC20_TOKEN_${index}_ADMIN_AUTHORITY_PRIVATE_KEY
  (
    cd "$ZEKO_UI_ROOT"
    ERC20_TOKEN_OWNER_PRIVATE_KEY=${!owner_private_var} \
      ERC20_TOKEN_VAULT_PRIVATE_KEY="$ERC20_SHARED_VAULT_PRIVATE_KEY" \
      ERC20_ADMIN_CONTRACT_PRIVATE_KEY=${!admin_contract_var} \
      ERC20_ADMIN_AUTHORITY_PRIVATE_KEY=${!admin_authority_var} \
      ERC20_IDENTITY_OUTPUT="$identity_file" \
      "$NIX" develop -c pnpm exec moon run eth-bridge-sdk:erc20-identity
  ) >/dev/null
  printf -v "ERC20_TOKEN_${index}_OWNER_PACKED" '%s' \
    "$(jq -er '.tokenOwnerPacked' "$identity_file")"
  printf -v "ERC20_TOKEN_${index}_TOKEN_ID" '%s' \
    "$(jq -er '.tokenIdHex' "$identity_file")"
done
ERC20_SHARED_VAULT_PACKED=$(jq -er '.tokenVaultPacked' "$TEMP_DIR/erc20-identity-0.json")

asset_domain=$("$CAST" keccak 'ZEKO_ERC20_ASSET_V1')
for index in 0 1; do
  address_var=ERC20_TOKEN_${index}_ADDRESS
  owner_var=ERC20_TOKEN_${index}_OWNER_PACKED
  token_id_var=ERC20_TOKEN_${index}_TOKEN_ID
  asset_preimage=$(
    "$CAST" abi-encode \
      'f(bytes32,uint256,address,address,bytes32,bytes32,uint8)' \
      "$asset_domain" 31337 "$BRIDGE_CONTRACT_ADDRESS" "${!address_var}" \
      "${!owner_var}" "${!token_id_var}" 9
  )
  printf -v "ERC20_TOKEN_${index}_ASSET_ID" '%s' \
    "$("$CAST" keccak "$asset_preimage" | tr '[:upper:]' '[:lower:]')"
done

printf -v ERC20_MFT_STANDARD_VK_ID '0x%064x' "$ERC20_MFT_STANDARD_VK_ID_DECIMAL"
printf -v ERC20_UNIVERSAL_BRIDGE_VK_ID '0x%064x' "$ERC20_UNIVERSAL_BRIDGE_VK_ID_DECIMAL"

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
      --ethereum-asset-registry-l2 "$ERC20_REGISTRY_L2" \
      --ethereum-shared-vault-l2 "$ERC20_SHARED_VAULT_L2" \
      --ethereum-mft-standard-vk-id "$ERC20_MFT_STANDARD_VK_ID_DECIMAL" \
      --ethereum-universal-bridge-vk-id "$ERC20_UNIVERSAL_BRIDGE_VK_ID_DECIMAL"
)

export BRIDGE_CONTRACT_ADDRESS ZEKO_CIRCUITS_CONFIG ZEKO_DEPLOY_CONFIG
export ERC20_REGISTRY_L2 ERC20_SHARED_VAULT_L2 ERC20_SHARED_VAULT_PACKED
export ERC20_SHARED_VAULT_PRIVATE_KEY ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY
export ERC20_MFT_STANDARD_VK_ID ERC20_UNIVERSAL_BRIDGE_VK_ID
export ERC20_TOKEN_0_ADDRESS ERC20_TOKEN_1_ADDRESS
export ERC20_TOKEN_0_ASSET_ID ERC20_TOKEN_1_ASSET_ID
export ERC20_TOKEN_0_OWNER_L2 ERC20_TOKEN_1_OWNER_L2
export ERC20_TOKEN_0_OWNER_PACKED ERC20_TOKEN_1_OWNER_PACKED
export ERC20_TOKEN_0_TOKEN_ID ERC20_TOKEN_1_TOKEN_ID
export ERC20_TOKEN_0_OWNER_PRIVATE_KEY ERC20_TOKEN_1_OWNER_PRIVATE_KEY
export ERC20_TOKEN_0_ADMIN_CONTRACT_PRIVATE_KEY ERC20_TOKEN_1_ADMIN_CONTRACT_PRIVATE_KEY
export ERC20_TOKEN_0_ADMIN_AUTHORITY_PRIVATE_KEY ERC20_TOKEN_1_ADMIN_AUTHORITY_PRIVATE_KEY
export ERC20_TOKEN_0_DEPOSIT_CAP ERC20_TOKEN_1_DEPOSIT_CAP
export ERC20_TOKEN_0_DEPOSIT_AMOUNT ERC20_TOKEN_1_DEPOSIT_AMOUNT

LIVE_ENV=$TEMP_DIR/live-deployment.env
{
  printf 'BRIDGE_CONTRACT_ADDRESS=%s\n' "$BRIDGE_CONTRACT_ADDRESS"
  printf 'MINA_SIGNING_NETWORK_ID=testnet\n'
  printf 'ZEKO_CIRCUITS_CONFIG=%s\n' "$ZEKO_CIRCUITS_CONFIG"
  printf 'ZEKO_DEPLOY_CONFIG=%s\n' "$ZEKO_DEPLOY_CONFIG"
} >"$LIVE_ENV"

echo "Running both standard-token mirrors through the live sequencer..."
BRIDGE_ASSET=erc20 POC_ENV_FILE="$LIVE_ENV" \
  BRIDGE_LIVE_FIXTURE_ROOT="$FIXTURE_ROOT" \
  BRIDGE_LIVE_STATE_DIR="$LIVE_DIR" \
  "$ROOT/tools/run-live-sequencer-bridge-e2e.sh"

scenario=$FIXTURE_ROOT/bridge-scenario.json
[[ $(jq -er '.ethereumAssets | length' "$scenario") == 2 ]]
for index in 0 1; do
  address_var=ERC20_TOKEN_${index}_ADDRESS
  asset_var=ERC20_TOKEN_${index}_ASSET_ID
  owner_var=ERC20_TOKEN_${index}_OWNER_PACKED
  token_id_var=ERC20_TOKEN_${index}_TOKEN_ID
  address_value=${!address_var}
  asset_value=${!asset_var}
  owner_value=${!owner_var}
  token_id_value=${!token_id_var}
  [[ $(jq -er ".ethereumAssets[$index].record.ethereumToken | ascii_downcase" "$scenario") == \
    "${address_value,,}" ]]
  [[ $(jq -er ".ethereumAssets[$index].record.assetId | ascii_downcase" "$scenario") == \
    "${asset_value,,}" ]]
  [[ $(jq -er ".ethereumAssets[$index].record.tokenOwnerL2 | ascii_downcase" "$scenario") == \
    "${owner_value,,}" ]]
  [[ $(jq -er ".ethereumAssets[$index].record.tokenIdL2 | ascii_downcase" "$scenario") == \
    "${token_id_value,,}" ]]
done

echo "Submitting the exact two-asset fixtures through Anvil custody..."
BRIDGE_ASSET=erc20 BRIDGE_FIXTURE_ROOT="$FIXTURE_ROOT" \
  "$ROOT/tools/run-local-bridge-roundtrip.sh"

jq -n --arg fixtures "$FIXTURE_ROOT" \
  --arg bridge "$BRIDGE_CONTRACT_ADDRESS" \
  --arg token0 "$ERC20_TOKEN_0_ADDRESS" --arg token1 "$ERC20_TOKEN_1_ADDRESS" \
  --arg assetId0 "$ERC20_TOKEN_0_ASSET_ID" --arg assetId1 "$ERC20_TOKEN_1_ASSET_ID" \
  '{status:"passed",bridgeAsset:"erc20",fixtures:$fixtures,
    bridge:$bridge,tokens:[$token0,$token1],assetIds:[$assetId0,$assetId1],
    sharedVault:true,universalBridgeVerificationKey:true,
    liveStandardTokenRoundtrip:true,anvilCustodyRoundtrip:true,
    registrationSettlements:1,bridgeSettlements:2,sp1ProofsGenerated:0}'
echo "Full local two-token ERC20 bridge roundtrip passed without generating an SP1 proof."
