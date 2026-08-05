# Command reference

Except for the explicitly approval-capped fixture benchmark, these commands
avoid proof generation.

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

This command creates two deterministic ERC-20 identities under one universal
registry configuration, settles and activates their ordered record batch,
deploys two unmodified Mina FungibleToken owner/admin pairs with a shared
proof-authorized vault, and replays both deposits and withdrawals through Anvil
custody. It uses the chain-ID-31337 mock verifier and does not request or
generate an SP1 proof.

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
cargo run --release --bin zkapp -- --execute --calculate-gas
cargo run --release --bin network_quote -- \
  --proof-system groth16 --pgu "$MAX_PGU"
```

`network_quote` reads auction parameters and never creates a request. The
current maximum quote is `baseFee + maxPricePerPgu * PGU`. Pass
`--include-balance` to also read the credited balance and requester address
derived from the existing `NETWORK_PRIVATE_KEY`; this still performs only
read-only RPCs. The `zkapp` command forces local CPU execution and calculates
settlement PGU without creating a network request; do not substitute the cycle
count from ordinary execute-only output.

## Approval-capped fixture benchmark

`network_fixture` executes and hashes an in-memory snapshot of the four pinned
Pickles fixture files. Without `--request`, it is a read-only preflight that
emits the snapshot's `inputSha256`, expected public-values hash, program vkey,
and local cycles:

```sh
cargo run --release --bin network_fixture
```

A paid Groth16 benchmark requires explicit approval of that exact digest and
all three cost caps:

```sh
cargo run --release --bin network_fixture -- \
  --request \
  --approved-input-sha256 "$INPUT_SHA256" \
  --max-pgu "$MAX_PGU" \
  --max-price-per-pgu "$MAX_PRICE_PER_PGU" \
  --max-total-atto-prove "$MAX_TOTAL_ATTO_PROVE"
```

The paid form requires `NETWORK_PRIVATE_KEY`, rechecks the snapshotted input and
live maximum cost, and does not retry submission when a fixed-nonce request has
an ambiguous outcome; it attempts read-only request recovery instead. Review
`cargo run --release --bin network_fixture -- --help` for the pinned fixture,
simulation, cap, timeout, and output defaults before approval. The saved proof
is a fixture-only benchmark artifact, not a Solidity-submittable settlement
receipt. Operational proofs use the
[gateway approval flow](/gateway/proving#approval-boundary).

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
