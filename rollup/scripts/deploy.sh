#!/usr/bin/env bash
set -euo pipefail

ROLLUP_NAME="${ZEKO_ROLLUP_NAME:-betternet}"
KEY_DIR="/data/keys"
CIRCUITS_DIR="/data/circuits"

export MINA_PRIVATE_KEY="$(cat "${KEY_DIR}/sequencer-sk")"
export ZEKO_DEPLOY_CONFIG="${CIRCUITS_DIR}/${ROLLUP_NAME}-deploy.json"
export ZEKO_CIRCUITS_CONFIG="${CIRCUITS_DIR}/${ROLLUP_NAME}-config.json"

SEQUENCER_PK="$(cat "${KEY_DIR}/sequencer-pk")"
FAUCET_PK="$(cat "${KEY_DIR}/faucet-pk")"
DA_LAYER_PK="$(cat "${KEY_DIR}/da-layer-pk")"

zeko-deploy deploy-all \
  --account-creation-fee "${ROLLUP_ACCOUNT_CREATION_FEE:-1}" \
  --da-keys "${DA_LAYER_PK}" \
  --da-quorum "${ROLLUP_DA_QUORUM:-1}" \
  --l1-uri "${ZEKO_L1_URI:-https://gateway.mina.devnet.zeko.io}" \
  --pause-key "${SEQUENCER_PK}" \
  --sequencer-key "${SEQUENCER_PK}" \
  --faucet-account "${FAUCET_PK}" \
  --da-node da-layer:1924

touch "${CIRCUITS_DIR}/.deployed"

echo "Rollup deployed."
