#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 <start|stop|status|logs> [testnet-dir]" >&2
  exit 2
}

[[ $# -ge 1 && $# -le 2 ]] || usage
COMMAND=$1
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DEPLOY_DIR=${2:-$ROOT/deploy/testnet}
NIX=${NIX:-$HOME/.nix-profile/bin/nix}
INDEXER_SESSION=zeko-actions-indexer
API_SESSION=zeko-actions-api

[[ $DEPLOY_DIR == /* ]] || DEPLOY_DIR="$ROOT/$DEPLOY_DIR"
DEPLOY_DIR=$(realpath "$DEPLOY_DIR")

load_config() {
  for file in "$DEPLOY_DIR/.env" "$DEPLOY_DIR/gateway.env"; do
    [[ -f $file ]] || {
      echo "Missing Actions service input: $file" >&2
      exit 1
    }
  done
  set -a
  source "$DEPLOY_DIR/.env"
  source "$DEPLOY_DIR/gateway.env"
  set +a
  : "${ZEKO_UI_ROOT:?set ZEKO_UI_ROOT in deploy/testnet/.env}"
  : "${ZEKO_UI_COMMIT:?set ZEKO_UI_COMMIT in deploy/testnet/.env}"
  : "${VIRTUAL_MINA_OUTER_PUBLIC_KEY:?materialize gateway.env first}"
  : "${ACTIONS_DB_PORT:=5434}"
  : "${ACTIONS_INDEXER_PORT:=9100}"
  : "${ACTIONS_API_BIND_ADDRESS:=127.0.0.1}"
  : "${ACTIONS_API_PORT:=9101}"
}

session_exists() {
  tmux has-session -t "$1" 2>/dev/null
}

stop_services() {
  for session in "$INDEXER_SESSION" "$API_SESSION"; do
    if session_exists "$session"; then
      tmux kill-session -t "$session"
    fi
  done
}

write_runtime_env() {
  local password database_url runtime_env
  password=$(tr -d '\r\n' <"$DEPLOY_DIR/secrets/postgres-gateway-password")
  database_url="postgresql://zeko_gateway:${password}@127.0.0.1:${ACTIONS_DB_PORT}/actions"
  runtime_env="$DEPLOY_DIR/secrets/actions-runtime.env"
  umask 077
  {
    printf 'DATABASE_URL=%s\n' "$database_url"
    printf 'AUTH_TOKEN=%s\n' "$(tr -d '\r\n' <"$DEPLOY_DIR/secrets/actions-indexer-token")"
    printf 'PORT=%s\n' "$ACTIONS_INDEXER_PORT"
    printf 'L1_ARCHIVE_URL=http://127.0.0.1:%s/graphql\n' "${GATEWAY_PORT:-8080}"
    printf 'L1_FINALITY=12\n'
    printf 'L2_ARCHIVE_URL=http://127.0.0.1:%s/graphql\n' "${SEQUENCER_PORT:-1923}"
    printf 'L2_FINALITY_TIME_H=1\n'
    printf 'OUTER_PK=%s\n' "$VIRTUAL_MINA_OUTER_PUBLIC_KEY"
    printf 'INNER_PK=%s\n' "$VIRTUAL_MINA_OUTER_PUBLIC_KEY"
    printf 'INDEX_OUTER=true\n'
    printf 'INDEX_INNER=false\n'
    printf 'ENVIRONMENT=local\n'
  } >"$runtime_env"
  chmod 0600 "$runtime_env"
}

run_in_ui_shell() {
  cd "$ZEKO_UI_ROOT"
  export PATH="$HOME/.proto/shims:$PATH"
  exec "$NIX" develop . --accept-flake-config -c "$@"
}

run_indexer() {
  load_config
  set -a
  source "$DEPLOY_DIR/secrets/actions-runtime.env"
  set +a
  mkdir -p "$DEPLOY_DIR/logs"
  exec > >(tee -a "$DEPLOY_DIR/logs/actions-indexer.log") 2>&1
  run_in_ui_shell pnpm exec moon run actions-indexer:start
}

run_api() {
  load_config
  mkdir -p "$DEPLOY_DIR/logs" "$DEPLOY_DIR/artifacts/wrangler-state"
  exec > >(tee -a "$DEPLOY_DIR/logs/actions-api.log") 2>&1
  run_in_ui_shell pnpm exec moon run actions-api:dev -- \
    --ip "$ACTIONS_API_BIND_ADDRESS" --port "$ACTIONS_API_PORT" \
    --env-file "$DEPLOY_DIR/secrets/actions-runtime.env" \
    --persist-to "$DEPLOY_DIR/artifacts/wrangler-state"
}

status_services() {
  load_config
  for session in "$INDEXER_SESSION" "$API_SESSION"; do
    if session_exists "$session"; then
      echo "$session: running"
    else
      echo "$session: stopped"
    fi
  done
  curl -fsS "http://127.0.0.1:$ACTIONS_INDEXER_PORT/health" || true
  echo
  curl -fsS "http://127.0.0.1:$ACTIONS_API_PORT/health" || true
  echo
}

start_services() {
  load_config
  for command in docker git tmux curl jq; do
    command -v "$command" >/dev/null || {
      echo "Missing command: $command" >&2
      exit 1
    }
  done
  [[ -x $NIX && -d $ZEKO_UI_ROOT/.git ]] || {
    echo "Missing Nix or Zeko UI checkout" >&2
    exit 1
  }
  [[ $(git -C "$ZEKO_UI_ROOT" rev-parse HEAD) == "$ZEKO_UI_COMMIT" ]] || {
    echo "Zeko UI checkout is not at pinned commit $ZEKO_UI_COMMIT" >&2
    exit 1
  }
  [[ -z $(git -C "$ZEKO_UI_ROOT" status --porcelain) ]] || {
    echo "Zeko UI checkout must be clean before starting retained services" >&2
    exit 1
  }
  for secret in postgres-gateway-password actions-indexer-token; do
    [[ -s "$DEPLOY_DIR/secrets/$secret" ]] || {
      echo "Missing secret: $DEPLOY_DIR/secrets/$secret" >&2
      exit 1
    }
  done
  if session_exists "$INDEXER_SESSION" || session_exists "$API_SESSION"; then
    echo "Actions tmux sessions already exist; run '$0 stop' first" >&2
    exit 1
  fi

  local compose=(docker compose --env-file "$DEPLOY_DIR/.env" \
    -f "$DEPLOY_DIR/compose.yaml")
  "${compose[@]}" up -d gateway-db
  local exists
  exists=$("${compose[@]}" exec -T gateway-db psql -U zeko_gateway \
    -d postgres -tAc "SELECT 1 FROM pg_database WHERE datname = 'actions'")
  if [[ $exists != 1 ]]; then
    "${compose[@]}" exec -T gateway-db createdb -U zeko_gateway actions
  fi

  write_runtime_env
  set -a
  source "$DEPLOY_DIR/secrets/actions-runtime.env"
  set +a
  (
    cd "$ZEKO_UI_ROOT"
    export PATH="$HOME/.proto/shims:$PATH"
    "$NIX" develop . --accept-flake-config -c \
      pnpm exec moon run actions-api:db-migrate
  )
  [[ $("${compose[@]}" exec -T gateway-db psql -U zeko_gateway -d actions \
    -tAc "SELECT to_regclass('public.outer_actions') IS NOT NULL") == t ]] || {
    echo "Actions database migration did not create outer_actions" >&2
    exit 1
  }

  tmux new-session -d -s "$INDEXER_SESSION" \
    "$ROOT/tools/machine-actions-services.sh _run-indexer '$DEPLOY_DIR'"
  tmux new-session -d -s "$API_SESSION" \
    "$ROOT/tools/machine-actions-services.sh _run-api '$DEPLOY_DIR'"
  echo "Actions services started in tmux. Run '$0 status' to check readiness."
}

case "$COMMAND" in
  start) start_services ;;
  stop) stop_services ;;
  status) status_services ;;
  logs)
    load_config
    tail -n 100 -F "$DEPLOY_DIR/logs/actions-indexer.log" \
      "$DEPLOY_DIR/logs/actions-api.log"
    ;;
  _run-indexer) run_indexer ;;
  _run-api) run_api ;;
  *) usage ;;
esac
