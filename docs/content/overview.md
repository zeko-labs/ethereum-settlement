# Overview

This repository is the Ethereum settlement and bridge glue for Zeko. It does
not reimplement Zeko in Rust or Solidity. The OCaml Zeko repository remains the
source of ledger, transaction, proving, action-state, bridge, and rollup
state-transition semantics.

The current milestone is a **Sepolia proof of concept with the existing 2-of-3
multisig DA path**. EIP-4844 blobs are intentionally deferred. A production
Ethereum design is expected to replace the committee DA path with blob-backed
batch data.

## What runs where

| Layer | Responsibility |
| --- | --- |
| OCaml Zeko | Sequences transactions, stores batches through multisig DA, produces Pickles proofs, and exports proof-bound settlement/bridge data. |
| Gateway | Implements the Mina GraphQL subset used by the sequencer, natively verifies settlement Pickles proofs, executes the smaller guests locally, obtains approved network proofs, submits Ethereum transactions, and indexes canonical Ethereum state. |
| SP1 settlement guest | Verifies the full Pickles proof and derives the Zeko outer-state receipt and optional inner-action claim tree. |
| SP1 bridge guest | Replays finalized native ETH and registered ERC-20 deposits into exact Zeko outer Witness actions and Poseidon checkpoints. |
| Ethereum contracts | Hold ETH and ERC-20 custody, verify SP1 proofs, enforce state continuity and slot bounds, activate proof-settled registry records, record checkpoints, and release delayed Merkle claims. |
| Succinct Network | Produces EVM-compatible Groth16 proofs after explicit operator approval. |

## End-to-end paths

Normal settlement:

```text
transaction -> sequencer -> 2-of-3 DA -> OCaml prover -> Pickles commit
  -> gateway GraphQL -> pinned native verification -> operator approval
  -> Succinct proof -> ZekoSettlement -> confirmed virtual Mina state
```

Bridge:

```text
Ethereum deposit -> finalized BridgeDeposit log -> bridge SP1 proof
  -> exact outer Witness action -> later OCaml settlement synchronizes deposit

OCaml inner withdrawal action -> settlement SP1 Keccak tree
  -> accepted settlement -> gateway Merkle path -> delayed ETH claim
```

The user does not generate a SNARK for withdrawal. The expensive binding is
performed once by the settlement prover; the user submits a fixed-depth
Keccak Merkle path to Ethereum.

## Repository map

| Path | Purpose |
| --- | --- |
| `program/settlement` | SP1 guest for Pickles verification and settlement receipts. |
| `program/bridge` | SP1 guest for canonical native and registered ERC-20 deposit batches. |
| `program/withdraw` | Legacy compatibility guest; disabled in the current native PoC. |
| `crates/pickles-verifier` | o1 `o1js-to-zkvm` Pickles verifier adapted to SP1. |
| `lib` | Shared, versioned host/guest public-value and witness types. |
| `api` | Gateway, Mina GraphQL façade, proof worker, and Ethereum indexer. |
| `contracts` | Upgradeable settlement and bridge contracts plus deployment scripts. |
| `deploy/testnet` | Persistent, pinned Compose reference profile for Sepolia. |
| `tools` | Artifact preparation, OCaml export, preflight, and local E2E scripts. |

Start with [current status](/status) before using the deployment runbook. The
PoC has a complete local path, but a retained release identity and live
Sepolia proof run are still operational launch work.
