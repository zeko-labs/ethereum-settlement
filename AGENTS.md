# Zeko Ethereum Settlement Agent Context

This repository is a proof-of-concept port of Zeko settlement from Mina to
Ethereum. Treat it as experimental glue code around the real Zeko implementation,
not as a replacement for the OCaml Zeko codebase.

## Target Architecture

- The OCaml Zeko repository remains the source of ledger, transaction, bridge,
  action-state, proving, and rollup state-transition semantics.
- SP1 verifies the Mina/Pickles proof emitted by the OCaml Zeko circuits.
- Ethereum verifies the SP1 proof in Solidity.
- Solidity should only own Ethereum-side custody, settlement checkpointing, and
  minimal bridge/DA glue.
- Ethereum blob DA is the intended production DA path. Do not carry over the
  Mina-side DA committee as the final Ethereum design.

## Current Settlement Shape

The current settlement PoC uses the o1 `o1js-to-zkvm` Pickles verifier path:

- `crates/pickles-verifier` is adapted from `~/zeko/o1js-to-zkvm`.
- `vendor/proof-systems` is the o1 SP1-compatible proof-systems branch.
- `program/settlement/build.rs` builds a verifier blob from
  `proofs/mainnet-blockchain-snark/vk.serde.json`.
- `program/settlement/src/main.rs` decodes the verifier blob, reads a
  `VerifiableProof`, calls `pickles_verifier::verify`, asserts the proof is
  valid, and commits `ZkappPublicValues`.
- `script/src/bin/main.rs --execute` uses `MinimalExecutorRunner` directly. This
  avoids an SP1 6.1 SDK issue where `client.execute(...).calculate_gas(false)`
  returns public values but later fails with `Failed to extract public value
  digest`.

Do not reintroduce the old hand-written Mina-Rust/Kimchi-only settlement path.
It was incomplete for Pickles because Pickles requires the recursive/deferred
checks and accumulator verification in addition to outer Kimchi verification.

## Known Working Command

The last known successful local execute command was:

```bash
cargo run --release --bin zkapp -- --execute
```

It completed with:

- `proof_valid: true`
- `cycles: 52146595101`
- `total gas: not calculated`

This command is CPU-heavy and can take around tens of minutes, but the direct
minimal-executor path should have much lower memory usage than the SP1 SDK
execute wrapper.

## Resource Safety

Be careful with SP1 workloads:

- Do not run local proving on a laptop without explicit approval.
- Do not run `--prove`, Groth16, PLONK, or network proof generation unless the
  user explicitly asks for it and the machine is sized for it.
- Prefer running heavy SP1 commands on a remote Linux machine with at least
  64 GB RAM; 128 GB+ is better for real proving.
- Use `tmux` and log output with `tee` for long runs.
- Monitor memory with `ps`, `htop`, or similar while running SP1 jobs.
- If disk gets full, remove generated build artifacts such as `target/debug`,
  `target/release`, `target/elf-compilation`, or temporary target directories.
  Never delete source, fixtures, or vendor directories unless asked.

## Toolchain

Expected tools:

- Rust stable via `rustup`
- Succinct/SP1 toolchain installed via `sp1up`
- `cargo +succinct --version` should work
- `cargo prove --version` should work
- `protoc`
- Go toolchain, because SP1 builds gnark-related components
- Docker/Foundry may be needed for Solidity/EVM paths

SP1 can build internal runner binaries under Cargo's registry cache. In sandboxed
environments this may require elevated filesystem access. If a build fails with
an error opening a `.cargo-lock` under
`~/.cargo/registry/src/.../sp1-core-executor-runner-*/target`, rerun the same
Cargo command outside the sandbox or with the appropriate approval.

## Verification Commands

Useful non-proving checks:

```bash
cargo check --release --offline -p zkapp-script
cargo check --offline -p settlement-program -p zkapp-script -p zeko_sp1_lib -p zeko-proof-api
cargo test --offline -p pickles-verifier
cargo fmt --all
git diff --check
```

`cargo fmt --all` may print warnings about nightly-only rustfmt options such as
`indent_style = Block` and `imports_granularity = Crate`. Those warnings are
currently expected.

## Settlement Binding Rules

The intended binding between Ethereum and the OCaml Zeko state transition is:

- L1 stores the expected Zeko verification-key hash.
- SP1 verifies the Mina/Pickles proof against that verification key.
- SP1 emits the verification-key hash in public values.
- Solidity compares the emitted hash to L1 state.

Do not add a separate Solidity account-update binding. The account update is
public input to the Pickles proof; once the proof and verification-key hash are
checked, that binding belongs inside the proof.

Important current limitation: the PoC `vk_hash` is currently a SHA-256 over the
fixture VK JSON bytes. Production must use the canonical OCaml/Mina
verification-key hash.

## Current Public Values Limitation

The current settlement public-values layout is still a PoC compatibility layout:

- `proof_valid`
- `vk_hash`
- `state_before[8]`
- `state_after[8]`
- `action_state_before`

The state and action-state fields are currently zeroed. Production work must
extract and commit the real Zeko outer state from the OCaml proof/public input:

- ledger hash
- inner action state and length
- synchronized outer action state and length
- current outer action-state checkpoint
- slot range
- DA metadata
- any bridge/account-set fields needed by Solidity safety checks

Version the public-values schema before treating it as production.

## Data Availability Direction

The intended Ethereum DA design is blob-based:

- settlement transactions should be tied to ordered EIP-4844 blob versioned
  hashes
- the OCaml state-transition proof should use a batch data root derived from the
  blob payload as public input
- SP1 should emit the DA root and blob hashes
- Solidity should check emitted blob hashes against the blobs attached to the
  settlement transaction

There is currently no complete blob DA implementation. Missing pieces include
blob payload encoding, blob transaction submission, Solidity `blobhash` checks,
DA-root binding, blob archival/indexing, and an equivalence proof between the
EIP-4844 blob commitments and the Zeko batch data root.

## Bridge Context

Read the original Zeko design docs in:

```text
~/zeko/zeko/src/app/zeko/circuits/design
```

Important semantics from the OCaml design:

- L1 to L2 communication is via outer `Witness` actions.
- L2 to L1 communication is via inner `Witness` actions.
- Sequencer commits emit `Commit` actions with ledger hash, inner action state,
  synchronized outer action state, action-state lengths, and slot bounds.
- Deposits are accepted only once a later accepted commit synchronizes them.
- Deposits can be cancelled if timeout wins before accepting commit.
- Withdrawals need inclusion in a committed inner action state plus delay rules.
- Helper accounts track processed deposit/withdrawal/cancel indices.

The Solidity bridge custody code and SP1 bridge accumulator code are PoC glue,
not the full Zeko bridge protocol.

## Stale Or Legacy Areas

- `contracts/src/fixtures/groth16-fixture.json` is legacy/stale until a fresh
  SP1 proof is generated.
- The current settlement fixtures are o1 example fixtures. Production fixtures
  must come from the OCaml Zeko state-transition prover.
- Do not restore removed old files such as the hand-written Mina-Rust parser,
  RKYV SRS blobs, or stale `proofs/*.txt`/`proofs/*.bin` files.

## Development Style

- Keep changes scoped. This repo is glue around OCaml Zeko, so avoid inventing
  replacement ledger or bridge semantics in Rust/Solidity.
- Prefer reusing o1 `o1js-to-zkvm` verifier code for Pickles verification.
- Prefer reusing OCaml Zeko outputs/fixtures for real state-transition data.
- Add negative tests for proof verification hardening when touching verifier
  logic.
- Do not silently change public-values layout or Solidity decoders without
  updating fixtures and documentation.
