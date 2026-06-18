#!/usr/bin/env bash
set -euo pipefail

ROLLUP_NAME="${ZEKO_ROLLUP_NAME:-betternet}"
KEY_DIR="/data/keys"
CIRCUITS_DIR="/data/circuits"

mkdir -p "${KEY_DIR}" "${CIRCUITS_DIR}"

zeko-cli generate-circuits-config \
  --circuits-config-output "${CIRCUITS_DIR}/${ROLLUP_NAME}-config.json" \
  --deploy-config-output "${CIRCUITS_DIR}/${ROLLUP_NAME}-deploy.json"

generate_keypair() {
  local name="$1"

  zeko-cli generate-even-key | while read -r label1 label2 value; do
    if [ "${label1}" = "Private" ]; then
      printf "%s" "${value}" > "${KEY_DIR}/${name}-sk"
    elif [ "${label1}" = "Public" ]; then
      printf "%s" "${value}" > "${KEY_DIR}/${name}-pk"
    fi
  done
}

generate_keypair sequencer
generate_keypair faucet
generate_keypair da-layer

touch "${KEY_DIR}/.keys_created"

echo "Rollup config and keys created."
echo "Sequencer public key: $(cat "${KEY_DIR}/sequencer-pk")"
echo "Faucet public key: $(cat "${KEY_DIR}/faucet-pk")"
echo "DA layer public key: $(cat "${KEY_DIR}/da-layer-pk")"
