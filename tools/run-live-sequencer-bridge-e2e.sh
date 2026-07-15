#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ZEKO_ROOT=${ZEKO_ROOT:-/root/zeko}
ZEKO_UI_ROOT=${ZEKO_UI_ROOT:-/root/zeko-ui}
ENV_FILE=${POC_ENV_FILE:-$ROOT/build/poc/deployment.env}
OUTPUT_DIR=${BRIDGE_LIVE_FIXTURE_ROOT:-$ROOT/build/poc/bridge-live-fixtures}
LIVE_DIR=${BRIDGE_LIVE_STATE_DIR:-$ROOT/build/poc/bridge-live-sequencer}
SEQUENCER_PORT=${SEQUENCER_PORT:-8082}
ARCHIVE_PROXY_PORT=${ARCHIVE_PROXY_PORT:-8083}
ACTIONS_INDEXER_PORT=${ACTIONS_INDEXER_PORT:-3602}
ACTIONS_API_PORT=${ACTIONS_API_PORT:-9102}
NIX=${NIX:-$HOME/.nix-profile/bin/nix}
CAST=${CAST:-$HOME/.foundry/bin/cast}
BUN=${BUN:-$HOME/.proto/tools/bun/1.2.22/bun}
ZEKO_LOG=${ZEKO_LIVE_LOG:-/tmp/zeko-live-bridge-$$.log}
ACTIONS_INDEXER_LOG=${ACTIONS_INDEXER_LOG:-/tmp/zeko-live-actions-indexer-$$.log}
ACTIONS_API_LOG=${ACTIONS_API_LOG:-/tmp/zeko-live-actions-api-$$.log}
ARCHIVE_PROXY_LOG=${ARCHIVE_PROXY_LOG:-/tmp/zeko-live-archive-proxy-$$.log}

for executable in "$NIX" "$CAST" "$BUN"; do
  [[ -x "$executable" ]] || {
    echo "Missing executable: $executable" >&2
    exit 1
  }
done
for command in curl docker jq nc pgrep; do
  command -v "$command" >/dev/null || {
    echo "Missing command: $command" >&2
    exit 1
  }
done
[[ -f "$ENV_FILE" ]] || {
  echo "Missing PoC deployment environment: $ENV_FILE" >&2
  exit 1
}
[[ -x "$ZEKO_ROOT/_build/default/src/app/zeko/sequencer/cli.exe" ]] || {
  echo "Build the Zeko sequencer before running the live integration" >&2
  exit 1
}
[[ -f "$ZEKO_UI_ROOT/packages/eth-bridge-sdk/moon.yml" ]] || {
  echo "Missing Zeko UI checkout: $ZEKO_UI_ROOT" >&2
  exit 1
}

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

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  [[ -z ${ACTIONS_INDEXER_PID:-} ]] || terminate_tree "$ACTIONS_INDEXER_PID"
  [[ -z ${ACTIONS_API_PID:-} ]] || terminate_tree "$ACTIONS_API_PID"
  [[ -z ${ARCHIVE_PROXY_PID:-} ]] || terminate_tree "$ARCHIVE_PROXY_PID"
  [[ -z ${ZEKO_PID:-} ]] || terminate_tree "$ZEKO_PID"
  docker rm -f pg-sequencer rabbitmq-sequencer >/dev/null 2>&1 || true
  if [[ $status -ne 0 ]]; then
    echo "Zeko log: $ZEKO_LOG" >&2
    echo "Actions indexer log: $ACTIONS_INDEXER_LOG" >&2
    echo "Actions API log: $ACTIONS_API_LOG" >&2
    echo "Archive proxy log: $ARCHIVE_PROXY_LOG" >&2
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

for port in 8080 "$SEQUENCER_PORT" "$ARCHIVE_PROXY_PORT" \
  "$ACTIONS_INDEXER_PORT" "$ACTIONS_API_PORT"; do
  if nc -z 127.0.0.1 "$port" 2>/dev/null; then
    echo "Port $port is already in use" >&2
    exit 1
  fi
done
if docker ps -a --format '{{.Names}}' | grep -qxE 'pg-sequencer|rabbitmq-sequencer'; then
  echo "A Zeko sequencer test container already exists" >&2
  exit 1
fi

set -a
source "$ENV_FILE"
set +a

rm -rf "$OUTPUT_DIR" "$LIVE_DIR"
mkdir -p "$OUTPUT_DIR" "$LIVE_DIR"
OUTPUT_DIR=$(realpath "$OUTPUT_DIR")
LIVE_DIR=$(realpath "$LIVE_DIR")
READY_FILE="$LIVE_DIR/ready.json"

if [[ -z ${ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY:-} ]]; then
  ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY=$(
    "$NIX" develop "git+file://$ZEKO_ROOT?submodules=1" --accept-flake-config -c \
      "$ZEKO_ROOT/_build/default/src/app/zeko/sequencer/cli.exe" \
      generate-even-key | awk -F': ' '/Private key:/ {print $2}'
  )
fi
[[ -n "$ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY" ]]

(
  cd "$ZEKO_ROOT"
  export ZEKO_ETHEREUM_SETTLEMENT_FIXTURE_DIR="$OUTPUT_DIR"
  ZEKO_ETHEREUM_BRIDGE_SCENARIO_DIR="$OUTPUT_DIR" \
    ZEKO_ETHEREUM_SETTLEMENT_FIXTURE_ONLY=true \
    ZEKO_ETHEREUM_BRIDGE_EXPORT_ONLY=true \
    ZEKO_ETHEREUM_BRIDGE_LIVE_SDK=true \
    ZEKO_ETHEREUM_BRIDGE_LIVE_DIR="$LIVE_DIR" \
    ZEKO_ETHEREUM_BRIDGE_LIVE_PORT="$SEQUENCER_PORT" \
    ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY="$ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY" \
    "$NIX" develop "git+file://$ZEKO_ROOT?submodules=1" --accept-flake-config -c \
      src/app/zeko/sequencer/tests/run-sequencer-test.sh real 1 false true
) >"$ZEKO_LOG" 2>&1 &
ZEKO_PID=$!

for _ in $(seq 1 1800); do
  [[ -f "$READY_FILE" ]] && break
  if ! kill -0 "$ZEKO_PID" 2>/dev/null; then
    echo "The live OCaml bridge harness exited before becoming ready" >&2
    exit 1
  fi
  sleep 1
done
[[ -f "$READY_FILE" ]] || {
  echo "Timed out waiting for live OCaml bridge harness" >&2
  exit 1
}
curl -fsS "http://127.0.0.1:$SEQUENCER_PORT/graphql" \
  -H 'content-type: application/json' \
  --data '{"query":"{ sequencerPk }"}' | jq -e '.data.sequencerPk' >/dev/null

for _ in $(seq 1 30); do
  if docker exec pg-sequencer pg_isready -U postgres >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec pg-sequencer createdb -U postgres actions
ACTIONS_DATABASE_URL=postgresql://postgres:postgres@127.0.0.1:5433/actions
(
  cd "$ZEKO_UI_ROOT"
  DATABASE_URL="$ACTIONS_DATABASE_URL" \
    "$NIX" develop -c pnpm exec moon run actions-api:db-migrate
) >/dev/null

outer_public_key=$(jq -er '.outerPublicKey' "$READY_FILE")
l1_graphql_url=$(jq -er '.l1GraphqlUrl' "$READY_FILE")
sequencer_graphql_url=$(jq -er '.sequencerGraphqlUrl' "$READY_FILE")
(
  MINA_ARCHIVE_TARGET="$l1_graphql_url" PORT="$ARCHIVE_PROXY_PORT" \
    "$BUN" "$ROOT/tools/mina-archive-compat-proxy.ts"
) >"$ARCHIVE_PROXY_LOG" 2>&1 &
ARCHIVE_PROXY_PID=$!
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:$ARCHIVE_PROXY_PORT/health" >/dev/null 2>&1; then
    break
  fi
  kill -0 "$ARCHIVE_PROXY_PID" 2>/dev/null || exit 1
  sleep 1
done
curl -fsS "http://127.0.0.1:$ARCHIVE_PROXY_PORT/health" >/dev/null
archive_proxy_graphql_url="http://127.0.0.1:$ARCHIVE_PROXY_PORT/graphql"
(
  cd "$ZEKO_UI_ROOT"
  DATABASE_URL="$ACTIONS_DATABASE_URL" AUTH_TOKEN=local-actions-token \
    PORT="$ACTIONS_INDEXER_PORT" L1_ARCHIVE_URL="$archive_proxy_graphql_url" \
    L1_FINALITY=0 L2_ARCHIVE_URL="$sequencer_graphql_url" L2_FINALITY_TIME_H=1 \
    OUTER_PK="$outer_public_key" INNER_PK="$outer_public_key" \
    INDEX_OUTER=true INDEX_INNER=false ENVIRONMENT=LOCAL \
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

deposit_aux=$(
  "$CAST" to-dec "$(jq -er '.depositAux' "$OUTPUT_DIR/bridge-scenario.json")"
)
for _ in $(seq 1 180); do
  indexed=$(jq -n --arg aux "$deposit_aux" \
    '{query:"query($input: OuterWitnessesFromAuxesInput!) { outerWitnessesFromAuxes(input:$input) { aux index } commitAsePastSlot(slot: 2147483647) { commit { index } } }",variables:{input:{auxes:[$aux]}}}' \
    | curl -fsS -H 'content-type: application/json' --data-binary @- \
        "http://127.0.0.1:$ACTIONS_API_PORT/graphql" 2>/dev/null || true)
  if [[ $(jq -r '.data.outerWitnessesFromAuxes | length // 0' <<<"$indexed") == 1 && \
        $(jq -r '.data.commitAsePastSlot.commit.index // empty' <<<"$indexed") != "" ]]; then
    break
  fi
  sleep 1
done
[[ $(jq -r '.data.outerWitnessesFromAuxes | length // 0' <<<"$indexed") == 1 ]]
jq -e '.data.commitAsePastSlot.commit.index != null' <<<"$indexed" >/dev/null

(
  cd "$ZEKO_UI_ROOT"
  LIVE_SEQUENCER_READY_FILE="$READY_FILE" \
    MINA_PRIVATE_KEY="$ZEKO_ETHEREUM_BRIDGE_RECIPIENT_PRIVATE_KEY" \
    ACTIONS_API_URL="http://127.0.0.1:$ACTIONS_API_PORT/graphql" \
    BRIDGE_ADDRESS="$BRIDGE_CONTRACT_ADDRESS" \
    DEPOSIT_AMOUNT_ZEKO="$(jq -er '.depositAmountZeko' "$OUTPUT_DIR/bridge-scenario.json")" \
    WITHDRAWAL_AMOUNT_ZEKO="$(jq -er '.withdrawalAmountZeko' "$OUTPUT_DIR/bridge-scenario.json")" \
    WITHDRAWAL_RECIPIENT="$(jq -er '.withdrawalRecipient' "$OUTPUT_DIR/bridge-scenario.json")" \
    "$NIX" develop -c pnpm exec moon run eth-bridge-sdk:live-sequencer-e2e
)

wait "$ZEKO_PID"
ZEKO_PID=
POC_REUSE_OCAML_EXPORT=true "$ROOT/tools/export-bridge-ocaml-fixtures.sh" \
  "$OUTPUT_DIR" >/dev/null

jq -n --slurpfile sdk "$LIVE_DIR/operations-complete" \
  --arg fixtures "$OUTPUT_DIR" \
  '{status:"passed",sdk:$sdk[0],fixtures:$fixtures,ocamlSettlements:2,
    liveSequencerGraphql:true,actionsPreparationApi:true,sp1ProofsGenerated:0}'
echo "Browser SDK -> live sequencer deposit finalization -> native withdrawal request passed."
echo "No SP1 proof was requested or generated."
