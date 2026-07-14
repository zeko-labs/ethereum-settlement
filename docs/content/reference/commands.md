# Command reference

All commands in the default sections avoid proof generation.

## Build and test

```sh
cargo check --offline \
  -p settlement-program -p zkapp-script -p zeko_sp1_lib -p zeko-proof-api
cargo test --offline -p pickles-verifier
cargo test --offline -p settlement-program -p bridge-program \
  -p withdraw-program -p zeko_sp1_lib -p zeko-proof-api

(cd contracts && forge build --sizes && forge test -vv)
(cd docs && pnpm install --frozen-lockfile && pnpm build)

cargo fmt --all -- --check
git diff --check
```

## Execute guests without proving

```sh
cargo run --release --bin zkapp -- --execute
cargo run --release --bin bridge -- --execute
cargo run --release --bin withdraw -- --execute
```

Use a genuine settlement fixture:

```sh
SETTLEMENT_VK_JSON="$PWD/fixtures/zeko-local-e2e/vk.serde.json" \
  cargo run --release --bin vkey
```

## Local native bridge

```sh
tools/export-bridge-ocaml-fixtures.sh build/poc/bridge-fixtures
tools/run-local-bridge-roundtrip.sh
```

Quick contract-only checkpoint:

```sh
cd contracts
forge test --match-path test/NativeBridgePocE2E.t.sol -vv
```

## Prepare deployment identity

```sh
FORGE=$HOME/.foundry/bin/forge \
  tools/prepare-poc.sh \
    "$RPC_URL" "$ADMIN_ADDRESS" \
    build/poc/bridge-fixtures/deposit-sync build/poc-sepolia
```

This builds/derives program vkeys and writes the manifest. It does not prove.

## Read prover-network pricing

```sh
cargo run --release --bin network_quote -- --proof-system groth16
cargo run --release --bin network_quote -- \
  --proof-system groth16 --pgu "$MAX_PGU"
```

The command reads auction parameters and never creates a request.

## Testnet profile

```sh
tools/testnet-preflight.sh deploy/testnet
docker compose --env-file deploy/testnet/.env \
  -f deploy/testnet/compose.yaml up -d
docker compose --env-file deploy/testnet/.env \
  -f deploy/testnet/compose.yaml logs -f gateway sequencer prover
```

## Inspect and approve a job

```sh
curl -H "x-api-key: $PROOF_API_KEY" \
  "$GATEWAY_URL/v1/proofs/$JOB_ID"

curl -H "x-api-key: $PROOF_API_KEY" \
  "$GATEWAY_URL/v1/proofs/$JOB_ID/quote?maxPgu=$MAX_PGU&maxPricePerPgu=$MAX_PRICE"
```

Approval is a paid-operation boundary. Use the reviewed command in [proof jobs
and approval](/gateway/proving), not an ad hoc request.
