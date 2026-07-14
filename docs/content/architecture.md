# Architecture

The gateway is the boundary between the OCaml rollup and Ethereum. It presents
Mina-shaped reads and mutations to the sequencer while translating accepted
commits into proof jobs and canonical Ethereum state.

```text
                                      +--------------------+
 users -> Zeko sequencer ------------>| 2-of-3 multisig DA |
              |                       +--------------------+
              +---- RabbitMQ ----> OCaml Pickles prover
              |                              |
              | sendZkapp + proof export     |
              v                              v
        +---------------- Zeko Ethereum gateway ----------------+
        | Mina GraphQL façade | PostgreSQL | canonical indexer   |
        | local SP1 preflight | approval   | proof/submission    |
        +-------------------------+------------------------------+
                                  |
                         Succinct Groth16 proof
                                  |
                                  v
                     +--------------------------+
 Ethereum users ---->| bridge proxy | settlement proxy |
                     +--------------------------+
```

## Component ownership

| Component | Owns | Does not own |
| --- | --- | --- |
| OCaml sequencer/prover | Transaction ordering, ledger transition, Pickles proof, action semantics, multisig DA writes. | Ethereum custody or SP1 request policy. |
| Gateway | Compatibility API, job persistence, local execution, proof approval, transaction submission, Ethereum-derived views. | Zeko ledger semantics or a trusted `stateAfter`. |
| Settlement guest | Complete Pickles verification and proof-bound receipt derivation. | Ethereum continuity or finality. |
| Bridge guest | Deposit accumulator replay and exact Poseidon outer actions. | Which logs are canonical; the gateway supplies only finalized contiguous logs. |
| Settlement contract | SP1 verification, outer-state continuity, action checkpoints, virtual slot, V2 inner root. | Pickles internals or L2 transaction execution. |
| Bridge contract | ETH custody, deposit accumulator, bridge proof acceptance, withdrawal delay/cursor, claim transfer. | L2 ledger or user-generated withdrawal proving. |

## Two directions of action flow

Zeko's outer account is the L1-facing side of the rollup and the inner account
is the L2-facing side.

- Ethereum-to-Zeko messages become **outer Witness actions**. The bridge proof
  appends them to the settlement contract's outer action checkpoint. They only
  become usable deposits after a later Pickles-proven commit synchronizes that
  checkpoint into the inner state.
- Zeko-to-Ethereum messages are **inner Witness actions**. The settlement guest
  proves their ordered transition and commits a Keccak root. The bridge
  releases ETH only for leaves under that settlement-bound root after delay.

## Serialized state writers

Bridge batches and sequencer settlements both advance the same outer action
state. The gateway therefore permits only one of them to validate, prove, or
submit at a time. Multiple settlements may wait in order, but Ethereum context
is assigned only when the worker claims a job after the previous writer is
confirmed.

This prevents paying for two proofs that both commit to the same starting batch
or action state.

## Canonical indexing and reorgs

The gateway records Ethereum blocks, proof submissions, virtual Mina account
snapshots, outer actions, bridge deposits, and inner-action leaves in
PostgreSQL. A job becomes confirmed only at the configured depth.

If a canonical block is replaced, the indexer restores the prior account and
action view, returns the affected command to the pending pool, and requeues the
existing paid proof request. It does not buy a second proof for the same input.

## Data availability boundary

For this milestone, the normal Zeko multisig DA stack remains in place with
three retained nodes and quorum two. Ethereum does not receive the full batch
payload. Blob publication, `blobhash` checks, archival, and an equivalence
proof between blob commitments and the Zeko batch root are future production
work.
