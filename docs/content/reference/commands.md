# Commands

## Execute programs

Execute the programs without generating proofs:

```sh
cargo run --release --bin zkapp -- --execute   # settlement program
cargo run --release --bin bridge -- --execute
cargo run --release --bin withdraw -- --execute
```

Use larger fixtures:

```sh
cargo run --release --bin bridge -- --execute --input proofs/bridge-input-200.json
cargo run --release --bin withdraw -- --execute --input proofs/withdraw-input-200.json
```

## Generate proofs

Generate a local proof without submitting to the network:

```sh
cargo run --release --bin bridge -- --prove
cargo run --release --bin withdraw -- --prove
```

Generate EVM-compatible proofs (Groth16 or PLONK) for on-chain submission:

```sh
cargo run --release --bin evm -- --system groth16
cargo run --release --bin evm -- --system plonk
```

Retrieve the settlement program verification key:

```sh
cargo run --release --bin vkey
```

To use the Succinct Prover Network instead of local proving:

```sh
SP1_PROVER=network NETWORK_PRIVATE_KEY=<key> cargo run --release --bin evm
```

## Run tests

```sh
cargo test -p bridge-program fixture_deposit_matches_zeko_action_state
cargo test -p withdraw-program
cd contracts && forge test --offline
```

## Run the o1js fixture

```sh
cd tools/zeko-action-state
npm install
npm start
```

## Bridge fixture checkpoint

`proofs/bridge-input.json` contains three deposits:

```text
before: 0x3772bc5435b957f81f86f752e93f2e29e886ac24580b3d1ec879c1dad26965f9
after : 0x3d638b908c4241e7b417d1790a79d0fe3277a133a5a87e12a484cd756de795bf
```
