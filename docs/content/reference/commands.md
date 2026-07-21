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
tools/run-live-sequencer-bridge-e2e.sh
tools/run-local-bridge-roundtrip.sh
```

The live-sequencer command uses real OCaml proving plus the Actions services and
browser SDK, but performs no SP1 proving. The round-trip command adds local
Ethereum custody, SP1 execution, settlement submission, and withdrawal claim.

## Local ERC-20 bridge

```sh
tools/run-local-erc20-bridge-roundtrip.sh
```

This single command creates a deterministic local ERC-20 identity, generates
the matching asset-specialized circuit configuration, deploys the unmodified
Mina FungibleToken owner/admin and proof-authorized vault through the live
sequencer, exports two real OCaml settlements, and replays the exact deposit and
withdrawal through Anvil custody. It uses the chain-ID-31337 mock verifier and
does not request or generate an SP1 proof.

Run the standalone browser app:

```sh
cd bridge-ui
pnpm install --frozen-lockfile
pnpm dev
```

It listens on `127.0.0.1:5174` and reads `public/runtime-config.json`.

Quick contract-only checkpoint:

```sh
cd contracts
forge test --match-path test/NativeBridgePocE2E.t.sol -vv
```

## Prepare deployment identity

Create the retained identities once:

```sh
tools/init-machine-testnet-identity.sh deploy/testnet
```

```sh
FORGE=$HOME/.foundry/bin/forge \
  tools/prepare-poc.sh \
    "$RPC_URL" "$ADMIN_ADDRESS" \
    build/poc/testnet-bridge-fixtures/deposit-sync build/poc-sepolia
```

This builds/derives program vkeys and writes the manifest. It does not prove.

Build and pin the machine-local runtime images:

```sh
tools/build-machine-images.sh \
  build/poc/testnet-bridge-fixtures/deposit-sync/vk.serde.json deploy/testnet
```

## Read prover-network pricing

```sh
cargo run --release --bin network_quote -- --proof-system groth16
cargo run --release --bin network_quote -- \
  --proof-system groth16 --pgu "$MAX_PGU"
```

The command reads auction parameters and never creates a request. The current
maximum quote is `baseFee + maxPricePerPgu * PGU`; obtain PGU from network
simulation rather than substituting local executor cycles.

## Testnet profile

```sh
tools/testnet-preflight.sh deploy/testnet
docker compose --env-file deploy/testnet/.env \
  -f deploy/testnet/compose.yaml up -d
docker compose --env-file deploy/testnet/.env \
  -f deploy/testnet/compose.yaml logs -f gateway sequencer prover

tools/machine-actions-services.sh start deploy/testnet
tools/machine-actions-services.sh status deploy/testnet
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
