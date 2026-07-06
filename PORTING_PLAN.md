# Zeko Ethereum Settlement Port: Gaps And Work Plan

This document tracks what has to be finished to turn the current Ethereum
settlement PoC into the intended Zeko-on-Ethereum architecture.

The target architecture is:

- The OCaml Zeko codebase remains the source of ledger, transaction, action,
  bridge, DA, and rollup transition semantics.
- SP1 verifies the Mina/Pickles proof produced by the OCaml Zeko circuits.
- Ethereum verifies the SP1 proof in Solidity.
- Ethereum blobs replace Zeko's Mina-side DA committee as the batch data
  availability layer.
- Solidity only owns Ethereum-side bridge custody, settlement checkpoints, and
  minimal glue needed to connect Ethereum deposits/claims to accepted Zeko state.

## Current PoC Boundary

The current repository has useful PoC pieces:

- `program/settlement` is intended to verify a supplied Zeko/o1 proof inside
  SP1 and emit public values. It builds the verifier index inside SP1 from the
  supplied verification key, checks the Pickles accumulator with a Vesta SRS,
  recomputes deferred values, runs Mina's proof-shape checks, verifies the
  outer Kimchi proof with a Pallas SRS, and emits settlement public values.
- `contracts/src/ZekoSettlement.sol` verifies the SP1 proof and checks the
  emitted Zeko verification-key hash.
- `program/bridge` and `program/withdraw` replay batches into the same
  Poseidon/Keccak action-state formulas used by the bridge PoC.
- `contracts/src/EthereumZekoBridge.sol` has real ETH/ERC20 custody,
  deposit-state, withdrawal-state, nullifier, and claim accounting.

It should still be treated as a PoC. The contract state is not yet a faithful
Ethereum representation of Zeko's real outer rollup state, and the bridge logic
does not yet implement the full bridge protocol described in the OCaml design
documents.

## Settlement Binding

### What is already the right shape

The L1 binding to the OCaml state transition should be by verification-key hash
and proof verification:

- The expected Zeko verification-key hash is stored in L1 state.
- The SP1 settlement guest must verify the Mina/Pickles proof.
- The SP1 public output includes the Zeko verification-key hash.
- `ZekoSettlement` compares that hash to the expected L1 value.

There is no need to separately bind the account update in Solidity. The account
update is already public input to the Pickles proof; once SP1 correctly verifies
the proof against the expected Zeko verification key, the account update binding
belongs inside the proof system, not in hand-written Solidity parsing.

### Missing production work

- Define a stable settlement public-values schema for the Zeko outer commit
  receipt.
- Track the real Zeko outer app state on Ethereum, not just `state[3]`.
- Include the Zeko fields that Ethereum bridge logic must rely on:
  - ledger hash
  - inner action state and length
  - synchronized outer action state and length
  - current outer action state checkpoint
  - commit slot range
  - relevant status/paused flags
  - account-set/hash fields if they are needed by bridge safety checks
- Version the public-values schema so SP1 guest, Solidity decoder, and fixtures
  cannot silently drift.
- Add tests that decode a real OCaml-produced commit and assert the exact L1
  state transition.

It is acceptable that this is not fully implemented yet, because the original
scope appears to have been a PoC. It must be implemented before this can be
treated as a real settlement bridge.

## Pickles Verification Gap

The settlement guest currently calls `kimchi::verifier::verify` directly after
manually constructing Pickles-style public inputs. That is not a complete
verification of a Mina/Pickles proof.

Pickles is a recursive layer on top of Kimchi. The Pickles protocol needs the
outer Kimchi proof verification plus the recursive/deferred computation checks
and the accumulator check. The accumulator is the `sg` / challenge-polynomial
commitment carried through the IPA opening proof and Pickles proof state.

The Mina Rust reference path for zkapp proofs is:

- `verify_zkapp(...)`
- `accumulator_check::accumulator_check(srs, &[sideloaded_proof])`
- `verify_impl(...)`
- `compute_deferred_values(proof)`
- `run_checks(proof, vk.index)`
- construct the prepared statement/public inputs
- call Kimchi verification
- accept only `accumulator_check && verified`

The reference accumulator check extracts:

- `proof.statement.proof_state.messages_for_next_wrap_proof
  .challenge_polynomial_commitment`
- `proof.statement.proof_state.deferred_values.bulletproof_challenges`

and then calls `batch_dlog_accumulator_check` against the Vesta SRS.

Implemented PoC path:

- the host sends only the verification key, proof, derived zkApp statement, and
  original zkApp command into SP1
- the guest derives the verifier index from the verification key and embedded
  Pallas SRS
- the guest calls `accumulator_check` with an embedded Vesta SRS
- the guest recomputes deferred values from the proof and runs Mina's
  `run_checks`
- the guest reconstructs the Kimchi public inputs from the recomputed deferred
  values and verifies the outer Kimchi proof

Remaining hardening work:

- Add negative tests that mutate deferred values, bulletproof challenges,
  `messages_for_next_wrap_proof.challenge_polynomial_commitment`, feature
  flags, and `prev_evals`; all must fail inside the SP1 guest.
- Replace the current single-proof deterministic accumulator batching with
  transcript-derived batching before accepting multi-proof settlement batches.

## Data Availability With Ethereum Blobs

The Ethereum port should not carry over Zeko's Mina-side DA committee design as
the production DA mechanism. The intended Ethereum design is to publish each L2
batch's transaction data / ledger diff payload into EIP-4844 blobs and bind the
settlement proof to those blob commitments.

This repo currently has no real blob DA path. There is no blob transaction
builder, no payload encoding, no `blobhash` check in Solidity, no DA commitment
in the settlement public values, and no archive/indexer path that can recover
Zeko state from posted blobs.

Important constraints:

- Solidity can read the versioned hash of each blob attached to the current
  transaction, but cannot read the blob bytes.
- Blob data is available through Ethereum consensus for a limited retention
  window, not stored forever as calldata would be.
- Therefore L1 can enforce that a settlement references the blobs attached to
  the settlement transaction, but it cannot inspect the batch payload directly.
- The simplest design is for the settlement call itself to be the blob-carrying
  transaction. If blobs are posted in a separate transaction, L1 needs a separate
  inclusion/binding protocol instead of a direct `blobhash(i)` check.
- Long-term reconstructability still requires Zeko-run or third-party archival
  storage of blob payloads.

The production settlement schema should include DA fields:

- ordered blob versioned hashes
- a Zeko batch data root over the decoded blob payload
- batch sequence number
- previous and next ledger hashes
- outer and inner action-state lengths covered by the batch
- encoding/schema version

The OCaml commit prover must use the same batch data root as a public input to
the Pickles proof. SP1 then verifies the Pickles proof and emits that DA root and
the ordered blob hashes as public values. Solidity verifies the SP1 proof and
checks that the emitted blob hashes match the blobs attached to the Ethereum
settlement transaction.

That last check is not enough by itself. The implementation also needs a
cryptographic equivalence proof that the Zeko batch data root is derived from the
same payload committed by the EIP-4844 blob KZG commitments. Otherwise a
settlement could post available but irrelevant blob data while proving a state
transition over different witness data. The equivalence can be handled either by
proving blob-payload/KZG consistency inside SP1, if that is affordable, or by
using the EIP-4844 point-evaluation precompile / an equivalent onchain protocol
to tie selected blob openings to the Zeko data-root construction.

Missing work:

- Specify the canonical blob payload format for Zeko batches:
  transactions, ledger diffs, imported Ethereum deposits, emitted withdrawals,
  action indices, slots, and any fee metadata.
- Define chunking across multiple blobs, ordering, padding, compression, and
  field-element alignment.
- Define the hash tree / data-root construction over decoded blob payloads and
  make OCaml, Rust, and Solidity fixtures agree on it.
- Define and implement the equivalence proof between EIP-4844 blob commitments
  and the Zeko batch data root.
- Add a blob transaction submitter to the settlement/batcher tooling.
- Add Solidity checks around `blobhash(i)` and store or emit the DA metadata
  needed by indexers.
- Add an archival service that fetches blobs within the retention window and
  stores enough data to reconstruct Zeko state.
- Define reorg/finality policy: when a blob-backed settlement is considered
  final, how the batcher retries, and what indexers do on reorg.
- Add end-to-end tests for blob submission, DA-root binding, SP1 settlement, and
  Solidity blob-hash checks.

Bridge implication: deposits originate in Ethereum contract state/logs, but the
Zeko batch that imports deposits should still be part of blob DA. Withdrawals
and ordinary L2 transactions need blob DA so users and provers can reconstruct
the ledger state and independently verify claim paths.

## Original Zeko Bridge Model

The current design docs in
`~/zeko/zeko/src/app/zeko/circuits/design` describe the bridge as a separate
token bridge layered on top of the rollup communication mechanism.

The important protocol facts are:

- The core rollup has an outer account on L1 and an inner account on L2.
- Communication from L1 to L2 is done by posting outer `Witness` actions.
- Communication from L2 to L1 is done by posting inner `Witness` actions.
- Sequencer commits emit `Commit` actions containing the ledger hash, inner
  action state, synchronized outer action state, action-state lengths, and slot
  bounds.
- Deposits are not accepted merely because the user posted a deposit action.
  They are accepted only if a later `Commit` action includes/synchronizes them
  before timeout.
- Cancelled deposits are the opposite path: the deposit can be reclaimed if
  timeout wins before an accepting commit.
- Withdrawals require proving that the withdrawal action is included in an
  inner action state committed by the rollup, plus any configured withdrawal
  delay.
- Helper accounts track the last processed deposit, withdrawal, and cancelled
  deposit indices to prevent double spends while allowing users to skip indices
  at their own risk.
- Bridge proof fees are part of the circuit-level account-update shape:
  one fee on submit, one fee on finalize, with fixed fee-recipient accounts.
- Bridge-side upgrade safety relies on verification-key hash checks between the
  token outer and token inner accounts.
- The historical `old/` docs add sequencer auction, slashing, staking, and
  upgrade ideas, but the design README marks those files as historical and not
  up to date.

## Bridge Gaps In This Repo

### Deposits

The current Ethereum deposit flow locks assets and appends an Ethereum deposit
accumulator. The SP1 bridge guest can replay that accumulator and compute a Zeko
action-state transition. That is useful, but it is not the full Zeko bridge
semantics yet.

Missing work:

- Decide the canonical replacement for the Mina L1 token-outer account in the
  Ethereum setting. The Ethereum bridge contract is the asset custodian, but the
  Zeko action consumed by OCaml still needs a canonical form and index.
- Bind deposit batches to accepted settlement checkpoints. A deposit action
  should not be considered consumable on Zeko merely because an SP1 bridge proof
  computes an action state; it must be tied to a Zeko outer action state that is
  later accepted by settlement.
- Track deposit indices and lengths in the same style as the OCaml bridge
  design, not just a nonce and accumulator.
- Implement timeout/cancelled-deposit semantics or explicitly declare them out
  of scope for the Ethereum bridge MVP.
- Add cancelled-deposit proof and Solidity refund path if timeouts are in scope.
- Add bridge proof-fee fields and checks if the Ethereum bridge is expected to
  preserve Zeko's fee model.
- Add tests against OCaml-generated bridge actions, not only Rust fixtures.
- Build an indexer/batcher that constructs deposit proof inputs from Ethereum
  logs and Zeko action checkpoints instead of accepting arbitrary JSON.

### Withdrawals

The current withdrawal path is closer to the desired shape because it requires
settlement action-state checkpoints and nullifies claims on Ethereum.

Missing work:

- Replace the generic action-state checkpoint checks with real outer-state /
  inner-action-state tracking from settlement.
- Enforce the withdrawal delay from the Zeko bridge spec if it remains part of
  the Ethereum design.
- Implement ERC20 withdrawal semantics in the SP1 withdraw guest; it currently
  only accepts the native token path.
- Track withdrawal indices in the bridge-compatible way instead of relying only
  on withdrawal roots and nullifiers.
- Test withdrawal batches generated from the OCaml bridge circuit.
- Decide how emergency/governance-induced inner-action-state changes affect
  pending withdrawals on Ethereum.

### Shared Bridge Work

- Define exact field encodings for Ethereum addresses, Mina public keys,
  amounts, timeouts, token IDs, and action indices.
- Ensure Solidity, Rust guests, host scripts, and OCaml agree on endianess.
- Version every hashed leaf/state prefix.
- Separate deposit and withdrawal replay/nullifier domains clearly.
- Add invariant tests for asset conservation across deposit, cancel, withdraw,
  and emergency paths.
- Document the trust model: admin roles are acceptable for the PoC, but every
  production role must be deliberately scoped and eventually timelocked or
  governed.

## Stale Code And Artifacts

Cleanup status:

- Template Fibonacci naming in host scripts has been removed.
- The unused `PoseidonMina.sol` proof wrapper is not present in the maintained
  path.
- The ad hoc root-level `raw.json`, `queryconverted.txt`, and
  `queryconverter.py` artifacts are not present.
- No unreferenced PLONK/Sui fixture JSON files remain outside vendored
  dependencies.
- `contracts/README.md` now describes the actual contracts and should be kept.
- CI/tooling still needs a full pass so it builds from the right directories
  with pinned host and SP1 toolchains.

Keep:

- `contracts/src/fixtures/groth16-fixture.json`, because the Solidity
  Groth16 fork test uses it.
- `proofs/*.json`, `proofs/*.bin`, `proofs/graphql.txt`, and `proofs/vk.txt`
  until replacement fixtures are generated from the OCaml flow.
- `tools/zeko-action-state`, because it is used as an action-state formula
  fixture even though it should eventually be replaced by OCaml-generated tests.

## Toolchain Requirements

Rust:

- `sp1-zkvm = 6.1.0` requires Rust newer than the repo's old implicit local
  stable. Pin the repo to Rust `1.92`.
- Keep release profile settings at the workspace root. Per-program profiles are
  ignored by Cargo in this workspace.

SP1:

- Current SP1 guests do not need `#![no_std]`. SP1 v6 guests use
  `#![no_main]` and `sp1_zkvm::entrypoint!(main)`.
- Pin SP1 to v6.1.0 for this repo.
- Use `cargo prove build --docker --tag v6.1.0` for reproducible guest builds,
  or ensure local `sp1up --version v6.1.0` installs a matching `succinct`
  toolchain.
- Pass `--rustflags=-C,passes=lower-atomic` when building SP1 guests. The
  RISC-V zkVM target does not provide the atomic runtime symbols pulled in by
  Mina/proof-system dependencies.
- `zeko_sp1_lib` includes zkVM-only `__atomic_*` shims for the single-threaded
  guest environment.
- `no_std` is not the current blocker. The blocker is guest safety of the
  dependency graph: proof-system code compiled for the SP1 guest must not reach
  Rayon/threading paths, unsupported optimized arkworks helpers, nondeterminism,
  or host-only assumptions.
- Do not mix old SP1 `succinct` toolchains with `sp1-zkvm 6.1.0`; the old
  local `rustc 1.85.0-dev` toolchain is incompatible.

Current settlement execution status:

- `cargo run --release --bin zkapp -- --execute` now completes successfully,
  verifies the Pickles proof, emits `proof_valid: true`, and prints
  `Pickles proof verified successfully`.
- The latest clean run used `22914842284` cycles and gas
  `Some(21120324386)`.
- Static Pallas and Vesta SRS data is embedded as aligned `include_bytes!`
  payloads. This avoids the previous `rkyv::access_unchecked` alignment bug.
- The guest now derives the verifier index from the supplied verification key
  and embedded Pallas SRS. The host no longer supplies deferred values or a
  serialized verifier index.
- The guest checks the Pickles Vesta accumulator, recomputes deferred values,
  runs Mina's proof-shape checks, reconstructs Kimchi public inputs, and verifies
  the outer Kimchi proof.
- The invalid-opcode failures found during execution were fixed in the local
  vendored forks:
  - serial zkVM paths for batch inversion, dense-polynomial evaluation, and
    polynomial helper loops
  - a direct cubic construction for Kimchi's permutation vanishing polynomial,
    avoiding a generic dense-polynomial multiplication path that emitted `UNIMP`
  - a SP1-specific Vesta MSM path for the accumulator check, using `sys_bigint`
    in canonical limb form
  - deterministic single-proof accumulator batching for the current PoC path
  - fixed public-value byte encoding with `commit_slice`
  - an explicit SP1 halt after committing public values, avoiding destructor
    teardown of large proof-system objects inside the guest
- Stale diagnostic markers and the obsolete byte-array MSM tests have been
  removed.

Known limitations:

- The accumulator batching path is intentionally single-proof and deterministic.
  Multi-proof batching still needs transcript-derived randomness.
- The SP1 compatibility patches live in local vendored forks. They need to be
  turned into a real upstreamable `zkvm`/SP1 feature profile.
- The execution is very expensive and has not been optimized.
- Negative tests for corrupted Pickles accumulator/deferred/public-input data
  are still missing.
- Host parsing still depends on Mina node/native GraphQL types, so the script
  build pulls more Mina crates than a minimal fixture parser should.

Required upstream/fork work:

- Add a real SP1/zkVM feature profile to the `proof-systems` and `algebra`
  forks, or add `#[cfg(target_os = "zkvm")]` serial alternatives in the exact
  verifier paths.
- Ensure the guest build disables or bypasses arkworks `parallel` feature paths
  for `ark-ec`, `ark-ff`, `ark-poly`, and `ark-std`.
- Keep the local MSM, batch inversion, polynomial evaluation, and Kimchi helper
  patches auditable and covered by regression tests.
- Re-run `zkapp --execute` after each dependency-level change to ensure the full
  Pickles verification still completes without `UNIMP`.

Foundry:

- Use Foundry nightly/current. Old local `forge 0.2.0` cannot parse current
  OpenZeppelin submodule config.
- CI should install Foundry via `foundry-rs/foundry-toolchain@v1` and run from
  `contracts`.

## Local Verification Targets

Minimum local targets before further protocol work:

```sh
cargo test -p bridge-program
cargo test -p withdraw-program
cargo test -p zeko_sp1_lib
cd contracts && forge test -vv
```

SP1 guest ELF builds:

```sh
cargo prove build --docker --tag v6.1.0 --locked \
  --rustflags=-C,passes=lower-atomic \
  -p settlement-program -p bridge-program -p withdraw-program
```

Settlement execution/proving should be rechecked after the guest dependency
graph is SP1-safe, after the public-values schema is finalized, and after the
fixtures are regenerated from OCaml output.

## Recommended Implementation Order

1. Clean stale artifacts and make the repo build/test with pinned toolchains.
2. Fix host/guest serialization drift and add regression tests for encodings.
3. Specify the settlement public-values schema as a versioned Rust/Solidity
   contract.
4. Specify the blob DA payload format and bind the settlement public values to
   blob versioned hashes plus the Zeko batch data root.
5. Make `ZekoSettlement` track the real Zeko outer state needed by the bridge
   and check the attached blob hashes for each settlement.
6. Regenerate settlement fixtures from the OCaml Zeko commit prover.
7. Redesign deposit proof inputs around accepted Zeko outer action checkpoints.
8. Add cancelled-deposit and timeout support, or explicitly cut them from the
   MVP with a written risk statement.
9. Complete withdrawal ERC20/index/delay semantics.
10. Replace ad hoc JSON job submission with an indexer/batcher that derives
    inputs from Ethereum logs, accepted Zeko checkpoints, and archived blob
    payloads.
11. Add end-to-end tests: Ethereum deposit -> blob-backed Zeko batch -> OCaml
    commit proof -> SP1 settlement -> Ethereum withdrawal claim.
