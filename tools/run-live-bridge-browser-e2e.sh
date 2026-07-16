#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BRIDGE_UI_ROOT=${BRIDGE_UI_ROOT:-$ROOT/bridge-ui}
EXPLORER_UI_ROOT=${EXPLORER_UI_ROOT:-$ROOT/explorer-ui}
ZEKO_ROOT=${ZEKO_ROOT:-/root/zeko}
DEPLOY_DIR=${BRIDGE_E2E_DEPLOY_DIR:-$ROOT/build/manual-stack/deploy}
ARTIFACT_DIR=${BRIDGE_E2E_ARTIFACT_DIR:-$ROOT/build/e2e/live-bridge}
GATEWAY_URL=${BRIDGE_E2E_GATEWAY_URL:-http://127.0.0.1:8080}
RPC_URL=${BRIDGE_E2E_RPC_URL:-http://127.0.0.1:8545}
SEQUENCER_URL=${BRIDGE_E2E_SEQUENCER_URL:-http://127.0.0.1:1923/graphql}
ACTIONS_URL=${BRIDGE_E2E_ACTIONS_URL:-http://127.0.0.1:9101/graphql}
BRIDGE_UI_URL=${BRIDGE_E2E_BRIDGE_UI_URL:-http://127.0.0.1:4174}
EXPLORER_UI_URL=${BRIDGE_E2E_EXPLORER_UI_URL:-http://127.0.0.1:4175}
ZEKO_CLI=${ZEKO_CLI:-$ZEKO_ROOT/_build/default/src/app/zeko/sequencer/cli.exe}
NIX=${NIX:-$HOME/.nix-profile/bin/nix}

for command in curl jq node pgrep; do
  command -v "$command" >/dev/null || {
    echo "Missing command: $command" >&2
    exit 1
  }
done
for executable in "$BRIDGE_UI_ROOT/node_modules/.bin/vite" \
  "$BRIDGE_UI_ROOT/node_modules/.bin/playwright" \
  "$EXPLORER_UI_ROOT/node_modules/.bin/vite"; do
  [[ -x $executable ]] || {
    echo "Missing UI executable: $executable" >&2
    exit 1
  }
done
[[ -f $DEPLOY_DIR/secrets/proof-api-key ]] || {
  echo "Missing proof API key in $DEPLOY_DIR" >&2
  exit 1
}

if [[ -z ${BRIDGE_E2E_ZEKO_PRIVATE_KEYS:-} ]]; then
  if [[ ${BRIDGE_E2E_REUSE_STACK:-false} == true ]]; then
    echo "Reuse mode requires BRIDGE_E2E_ZEKO_PRIVATE_KEYS for two prefunded Zeko accounts." >&2
    exit 1
  fi
  [[ -x $ZEKO_CLI ]] || {
    "$NIX" develop "git+file://$ZEKO_ROOT?submodules=1" --accept-flake-config -c \
      bash -lc 'cd "$1" && dune build src/app/zeko/sequencer/cli.exe' bash "$ZEKO_ROOT"
  }
  generate_key() {
    env -u ZEKO_CIRCUITS_CONFIG -u ZEKO_DEPLOY_CONFIG \
      "$ZEKO_CLI" generate-even-key | awk -F': ' '/Private key:/ {print $2}'
  }
  BRIDGE_E2E_ZEKO_PRIVATE_KEYS="$(generate_key),$(generate_key)"
fi
export BRIDGE_E2E_ZEKO_PRIVATE_KEYS
IFS=, read -r -a zeko_e2e_keys <<<"$BRIDGE_E2E_ZEKO_PRIVATE_KEYS"
if [[ ${#zeko_e2e_keys[@]} -ne 2 || -z ${zeko_e2e_keys[0]} || -z ${zeko_e2e_keys[1]} || \
      ${zeko_e2e_keys[0]} == "${zeko_e2e_keys[1]}" ]]; then
  echo "BRIDGE_E2E_ZEKO_PRIVATE_KEYS must contain two distinct comma-separated keys." >&2
  exit 1
fi
BRIDGE_E2E_ZEKO_PUBLIC_KEYS=$(
  cd "$BRIDGE_UI_ROOT"
  node --input-type=module -e '
    import { PrivateKey } from "o1js"
    const values = process.env.BRIDGE_E2E_ZEKO_PRIVATE_KEYS.split(",")
    process.stdout.write(values.map((value) => PrivateKey.fromBase58(value).toPublicKey().toBase58()).join(","))
  '
)
export BRIDGE_E2E_ZEKO_PUBLIC_KEYS

mkdir -p "$ARTIFACT_DIR"
ARTIFACT_DIR=$(realpath "$ARTIFACT_DIR")

terminate_tree() {
  local pid=$1 child
  while read -r child; do
    [[ -n $child ]] && terminate_tree "$child"
  done < <(pgrep -P "$pid" 2>/dev/null || true)
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  [[ -z ${BRIDGE_PREVIEW_PID:-} ]] || terminate_tree "$BRIDGE_PREVIEW_PID"
  [[ -z ${EXPLORER_PREVIEW_PID:-} ]] || terminate_tree "$EXPLORER_PREVIEW_PID"
  [[ -z ${STACK_PID:-} ]] || terminate_tree "$STACK_PID"
  if [[ $status -ne 0 ]]; then
    echo "Live bridge E2E failed; artifacts and logs: $ARTIFACT_DIR" >&2
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

if [[ ${BRIDGE_E2E_REUSE_STACK:-false} != true ]]; then
  [[ -n ${BRIDGE_E2E_STACK_COMMAND:-} ]] || {
    echo "Set BRIDGE_E2E_STACK_COMMAND to a foreground full-stack launcher, or BRIDGE_E2E_REUSE_STACK=true." >&2
    exit 1
  }
  (
    cd "$ROOT"
    exec bash -lc "$BRIDGE_E2E_STACK_COMMAND"
  ) >"$ARTIFACT_DIR/stack.log" 2>&1 &
  STACK_PID=$!
fi

wait_http() {
  local label=$1 url=$2 body=${3:-}
  for _ in $(seq 1 1800); do
    if [[ -n $body ]]; then
      curl -fsS -H 'content-type: application/json' --data "$body" "$url" >/dev/null 2>&1 && return
    else
      curl -fsS "$url" >/dev/null 2>&1 && return
    fi
    [[ -z ${STACK_PID:-} ]] || kill -0 "$STACK_PID" 2>/dev/null || {
      echo "$label stack process exited before readiness" >&2
      exit 1
    }
    sleep 1
  done
  echo "Timed out waiting for $label at $url" >&2
  exit 1
}

wait_http gateway "$GATEWAY_URL/health"
wait_http sequencer "$SEQUENCER_URL" '{"query":"query E2EHealth { sequencerPk }"}'
wait_http actions "$ACTIONS_URL" '{"query":"query E2EActions { __typename }"}'
wait_http explorer "$GATEWAY_URL/v1/explorer/summary"

chain_id_hex=$(curl -fsS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' \
  "$RPC_URL" | jq -er '.result')
chain_id=$((chain_id_hex))
[[ $chain_id == 31337 ]] || {
  echo "The live browser suite requires isolated Anvil chain 31337, got $chain_id" >&2
  exit 1
}
accounts=$(curl -fsS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_accounts","params":[]}' \
  "$RPC_URL" | jq -er '.result | length')
((accounts >= 2)) || {
  echo "Anvil must expose at least two unlocked accounts" >&2
  exit 1
}

(
  cd "$BRIDGE_UI_ROOT"
  "$BRIDGE_UI_ROOT/node_modules/.bin/vite" build
  BRIDGE_UI_GATEWAY_URL="$GATEWAY_URL" \
    BRIDGE_UI_SEQUENCER_GRAPHQL_URL="$SEQUENCER_URL" \
    BRIDGE_UI_ZEKO_ARCHIVE_GRAPHQL_URL="$GATEWAY_URL/archive/graphql" \
    BRIDGE_UI_ACTIONS_API_URL="$ACTIONS_URL" \
    BRIDGE_UI_ETHEREUM_CHAIN_ID=31337 \
    BRIDGE_UI_ETHEREUM_EXPLORER_URL="$EXPLORER_UI_URL" \
    BRIDGE_UI_ZEKO_EXPLORER_URL="$EXPLORER_UI_URL" \
    BRIDGE_UI_POLL_INTERVAL_MS=1000 \
    node scripts/write-runtime-config.mjs dist/runtime-config.json
)
(
  cd "$EXPLORER_UI_ROOT"
  "$EXPLORER_UI_ROOT/node_modules/.bin/vite" build
  jq -n --arg gateway "$GATEWAY_URL" --arg bridge "$BRIDGE_UI_URL" \
    --arg ethereum "$EXPLORER_UI_URL" \
    '{schemaVersion:1,gatewayUrl:$gateway,bridgeUiUrl:$bridge,
      ethereumExplorerUrl:$ethereum,networkName:"Zeko Local / Anvil",pollIntervalMs:1000}' \
    >dist/runtime-config.json
)

(cd "$BRIDGE_UI_ROOT" && exec "$BRIDGE_UI_ROOT/node_modules/.bin/vite" preview \
  --host 127.0.0.1 --port "${BRIDGE_UI_URL##*:}" --strictPort) >"$ARTIFACT_DIR/bridge-ui.log" 2>&1 &
BRIDGE_PREVIEW_PID=$!
(cd "$EXPLORER_UI_ROOT" && exec "$EXPLORER_UI_ROOT/node_modules/.bin/vite" preview \
  --host 127.0.0.1 --port "${EXPLORER_UI_URL##*:}" --strictPort) >"$ARTIFACT_DIR/explorer-ui.log" 2>&1 &
EXPLORER_PREVIEW_PID=$!
wait_http bridge-ui "$BRIDGE_UI_URL/runtime-config.json"
wait_http explorer-ui "$EXPLORER_UI_URL/runtime-config.json"

export BRIDGE_E2E_GATEWAY_URL="$GATEWAY_URL"
export BRIDGE_E2E_RPC_URL="$RPC_URL"
export BRIDGE_E2E_BRIDGE_UI_URL="$BRIDGE_UI_URL"
export BRIDGE_E2E_EXPLORER_UI_URL="$EXPLORER_UI_URL"
export BRIDGE_E2E_ARTIFACT_DIR="$ARTIFACT_DIR"
export BRIDGE_E2E_PROOF_API_KEY
BRIDGE_E2E_PROOF_API_KEY=$(tr -d '\r\n' <"$DEPLOY_DIR/secrets/proof-api-key")

cd "$BRIDGE_UI_ROOT"
"$BRIDGE_UI_ROOT/node_modules/.bin/playwright" test \
  --config playwright.live.config.ts
