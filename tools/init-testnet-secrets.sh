#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 <final-bridge-proxy-address> [testnet-dir]" >&2
  exit 2
}

[[ $# -ge 1 && $# -le 2 ]] || usage
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ZEKO_ROOT=${ZEKO_ROOT:-/root/zeko}
TESTNET_DIR=${2:-$ROOT/deploy/testnet}
ENV_TEMPLATE=$ROOT/deploy/testnet/.env.example
BRIDGE_ADDRESS=$1
NIX=${NIX:-$HOME/.nix-profile/bin/nix}
CAST=${CAST:-$HOME/.foundry/bin/cast}

[[ $BRIDGE_ADDRESS =~ ^0x[0-9a-fA-F]{40}$ ]] || {
  echo "Bridge address must be a 20-byte 0x-prefixed value" >&2
  exit 1
}
[[ -x $NIX && -x $CAST && -d $ZEKO_ROOT ]] || {
  echo "Missing Nix, cast, or Zeko checkout" >&2
  exit 1
}
for command in awk jq openssl; do
  command -v "$command" >/dev/null || {
    echo "Missing command: $command" >&2
    exit 1
  }
done

[[ $TESTNET_DIR == /* ]] || TESTNET_DIR="$ROOT/$TESTNET_DIR"
mkdir -p "$TESTNET_DIR/config" "$TESTNET_DIR/secrets"
TESTNET_DIR=$(realpath "$TESTNET_DIR")
for path in "$TESTNET_DIR/.env" "$TESTNET_DIR/config/circuits.json" \
  "$TESTNET_DIR/secrets/fixture-keys.env" \
  "$TESTNET_DIR/secrets/zeko-deploy-config.json"; do
  [[ ! -e $path ]] || {
    echo "Refusing to overwrite existing testnet identity: $path" >&2
    exit 1
  }
done

umask 077
"$NIX" develop "git+file://$ZEKO_ROOT?submodules=1" --accept-flake-config -c \
  bash -lc 'cd "$1" && dune build src/app/zeko/sequencer/cli.exe' bash \
  "$ZEKO_ROOT"
ZEKO_CLI="$ZEKO_ROOT/_build/default/src/app/zeko/sequencer/cli.exe"

run_zeko_cli() {
  env -u ZEKO_CIRCUITS_CONFIG -u ZEKO_DEPLOY_CONFIG \
    -u ZEKO_ETHEREUM_BRIDGE_ADDRESS "$ZEKO_CLI" "$@"
}

generate_even_key() {
  local output private_key public_key
  output=$(run_zeko_cli generate-even-key)
  private_key=$(awk -F': ' '/Private key:/ {print $2}' <<<"$output")
  public_key=$(awk -F': ' '/Public key:/ {print $2}' <<<"$output")
  [[ -n $private_key && -n $public_key ]]
  printf '%s\t%s\n' "$private_key" "$public_key"
}

IFS=$'\t' read -r sequencer_private sequencer_public < <(generate_even_key)
IFS=$'\t' read -r da1_private da1_public < <(generate_even_key)
IFS=$'\t' read -r da2_private da2_public < <(generate_even_key)
IFS=$'\t' read -r da3_private da3_public < <(generate_even_key)
IFS=$'\t' read -r bridge_recipient_private bridge_recipient_public < <(generate_even_key)

[[ $da1_public != "$da2_public" && $da1_public != "$da3_public" && \
   $da2_public != "$da3_public" ]] || {
  echo "Generated duplicate DA keys" >&2
  exit 1
}

run_zeko_cli generate-circuits-config \
  --ethereum-bridge-address "$BRIDGE_ADDRESS" \
  --circuits-config-output "$TESTNET_DIR/config/circuits.json" \
  --deploy-config-output "$TESTNET_DIR/secrets/zeko-deploy-config.json"

write_secret() {
  printf '%s\n' "$2" >"$TESTNET_DIR/secrets/$1"
  chmod 0600 "$TESTNET_DIR/secrets/$1"
}

gateway_private_key="0x$(openssl rand -hex 32)"
gateway_prover_address=$(
  "$CAST" wallet address --private-key "$gateway_private_key"
)
network_private_key="0x$(openssl rand -hex 32)"
network_requester_address=$(
  "$CAST" wallet address --private-key "$network_private_key"
)
write_secret proof-api-key "$(openssl rand -hex 32)"
write_secret actions-indexer-token "$(openssl rand -hex 32)"
write_secret network-private-key "$network_private_key"
write_secret settlement-private-key "$gateway_private_key"
write_secret bridge-private-key "$gateway_private_key"
write_secret withdraw-private-key "$gateway_private_key"
write_secret postgres-gateway-password "$(openssl rand -hex 32)"
write_secret postgres-sequencer-password "$(openssl rand -hex 32)"
write_secret rabbitmq-password "$(openssl rand -hex 32)"
write_secret sequencer-private-key "$sequencer_private"
write_secret sequencer-signer-token "$(openssl rand -hex 32)"
write_secret da1-private-key "$da1_private"
write_secret da1-signer-token "$(openssl rand -hex 32)"
write_secret da2-private-key "$da2_private"
write_secret da2-signer-token "$(openssl rand -hex 32)"
write_secret da3-private-key "$da3_private"
write_secret da3-signer-token "$(openssl rand -hex 32)"
write_secret bridge-recipient-private-key "$bridge_recipient_private"

openssl req -x509 -newkey rsa:3072 -nodes -days 30 \
  -keyout "$TESTNET_DIR/secrets/signer-tls.key" \
  -out "$TESTNET_DIR/secrets/signer-tls.crt" \
  -subj '/CN=zeko-testnet-signers' \
  -addext 'subjectAltName=DNS:sequencer-signer,DNS:da1-signer,DNS:da2-signer,DNS:da3-signer' \
  >/dev/null 2>&1
chmod 0600 "$TESTNET_DIR/secrets/signer-tls.key" \
  "$TESTNET_DIR/secrets/signer-tls.crt" \
  "$TESTNET_DIR/secrets/zeko-deploy-config.json"

awk -v da="$da1_public,$da2_public,$da3_public" \
  -v sequencer="$sequencer_public" \
  -v bridge_recipient="$bridge_recipient_public" '
    /^DA_PUBLIC_KEYS=/ { print "DA_PUBLIC_KEYS=" da; next }
    /^SEQUENCER_PUBLIC_KEY=/ { print "SEQUENCER_PUBLIC_KEY=" sequencer; next }
    /^BRIDGE_RECIPIENT_PUBLIC_KEY=/ {
      print "BRIDGE_RECIPIENT_PUBLIC_KEY=" bridge_recipient; next
    }
    { print }
  ' "$ENV_TEMPLATE" >"$TESTNET_DIR/.env"
chmod 0600 "$TESTNET_DIR/.env"

{
  printf 'ZEKO_CIRCUITS_CONFIG=%q\n' "$TESTNET_DIR/config/circuits.json"
  printf 'ZEKO_DEPLOY_CONFIG=%q\n' \
    "$TESTNET_DIR/secrets/zeko-deploy-config.json"
  printf 'ZEKO_ETHEREUM_BRIDGE_ADDRESS=%q\n' "$BRIDGE_ADDRESS"
  printf 'BRIDGE_CONTRACT_ADDRESS=%q\n' "$BRIDGE_ADDRESS"
  printf 'ZEKO_ETHEREUM_COMMIT_VALIDITY_PERIOD=2400\n'
  printf 'ZEKO_TEST_L1_NETWORK_ID=testnet\n'
  printf 'MINA_SIGNING_NETWORK_ID=testnet\n'
  printf 'ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY=%q\n' \
    "$bridge_recipient_private"
  printf 'ZEKO_ETHEREUM_WITHDRAWAL_RECIPIENT=%q\n' \
    "$gateway_prover_address"
  printf 'ZEKO_TEST_SEQUENCER_SIGNER_PRIVATE_KEY=%q\n' "$sequencer_private"
  printf 'DA1_SIGNER_PRIVATE_KEY=%q\n' "$da1_private"
  printf 'DA2_SIGNER_PRIVATE_KEY=%q\n' "$da2_private"
  printf 'DA3_SIGNER_PRIVATE_KEY=%q\n' "$da3_private"
} >"$TESTNET_DIR/secrets/fixture-keys.env"
chmod 0600 "$TESTNET_DIR/secrets/fixture-keys.env"

jq -n --arg directory "$TESTNET_DIR" --arg sequencer "$sequencer_public" \
  --arg da1 "$da1_public" --arg da2 "$da2_public" --arg da3 "$da3_public" \
  --arg bridge "$BRIDGE_ADDRESS" --arg gatewayProver "$gateway_prover_address" \
  --arg networkRequester "$network_requester_address" \
  --arg bridgeRecipient "$bridge_recipient_public" \
  --arg minaSigningNetworkId testnet \
  '{directory:$directory,bridge:$bridge,sequencerPublicKey:$sequencer,
    daPublicKeys:[$da1,$da2,$da3],gatewayProverAddress:$gatewayProver,
    networkRequesterAddress:$networkRequester,bridgeRecipientPublicKey:$bridgeRecipient,
    minaSigningNetworkId:$minaSigningNetworkId,
    next:"fill immutable image digests, fund role/requester keys, source secrets/fixture-keys.env, then export the bridge fixtures"}'
