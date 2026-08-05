# Verification

Use the smallest tier that covers the change. The default suite does not
generate an SP1 proof.

## Rust and verifier

```sh
cargo check --offline \
  -p settlement-program \
  -p zkapp-script \
  -p zeko_sp1_lib \
  -p zeko-proof-api

cargo test --offline -p pickles-verifier
cargo test --offline -p settlement-program
cargo test --offline -p bridge-program
cargo test --offline -p withdraw-program
cargo test --offline -p zeko_sp1_lib
cargo test --offline -p zeko-proof-api
```

The Pickles suite includes negative mutations of application statements,
deferred values, bulletproof challenges, accumulator points, feature flags,
previous evaluations, body fields, and action bindings.

## Solidity

```sh
cd contracts
forge build --sizes
forge test -vv
```

For the focused native bridge protocol checkpoint:

```sh
forge test --match-path test/NativeBridgePocE2E.t.sol -vv
```

## OCaml cross-language checks

```sh
cd ../zeko
nix develop "git+file://$PWD?submodules=1" --accept-flake-config \
  -c dune exec src/app/zeko/tests/ethereum_bridge_vectors.exe
```

This asserts that OCaml and Rust agree on the synthetic Ethereum deposit
holder, Poseidon aux values, and action encodings.

## Deployment/static checks

```sh
docker compose --env-file deploy/testnet/.env \
  -f deploy/testnet/compose.yaml config --quiet

bash -n tools/*.sh
cargo fmt --all -- --check
git diff --check
```

Rustfmt can warn about nightly-only settings such as `imports_granularity` and
`indent_style`; the warning is expected, formatting differences are not.

Build the docs:

```sh
cd docs
pnpm install --frozen-lockfile
pnpm build
```

## Heavy execute-only checkpoint

Run this after verifier, serialization, or receipt changes:

```sh
cargo run --release --bin zkapp -- --execute
```

For the genuine native flow, use [the full local E2E](/operations/local-e2e).
These commands execute SP1 but do not prove.

## What requires explicit intent

The following are not routine tests:

- `--prove`
- `cargo run --release --bin evm`
- `network_fixture --request`
- `SP1_PROVER=network`
- gateway approval of a job
- a broadcast Foundry script against Sepolia

They consume substantial resources, funds, or external state. Run them only as
part of the reviewed testnet runbook.
