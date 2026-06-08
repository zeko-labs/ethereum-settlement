# Overview

Zeko Ethereum L2 uses SP1 proofs to connect Zeko state transitions with
Ethereum contracts.

The project contains three independent SP1 programs:

| Program | Direction | Purpose |
| --- | --- | --- |
| `program/settlement` | Zeko to Ethereum | Verifies a Zeko/o1 zkApp proof and exposes the rollup root transition. |
| `program/bridge` | Ethereum to Zeko | Replays an ordered batch of Ethereum deposits and computes the matching Zeko action-state transition. |
| `program/withdraw` | Zeko to Ethereum | Replays an ordered batch of Zeko withdrawal actions and computes the matching Ethereum withdrawal accumulator. |

The SP1 programs perform expensive proof verification and hashing off-chain.
The Ethereum contracts verify succinct SP1 proofs and enforce continuity
against state already stored on Ethereum.

## Repository layout

| Path | Purpose |
| --- | --- |
| `program/settlement` | Settlement SP1 guest program. |
| `program/bridge` | Deposit bridge SP1 guest program. |
| `program/withdraw` | Withdrawal SP1 guest program. |
| `lib` | Shared Rust input and output types. |
| `script` | Host-side execution and proof-generation binaries. |
| `contracts/src/ZekoSettlement.sol` | Ethereum settlement verifier and action-state checkpoint registry. |
| `contracts/src/EthereumZekoBridge.sol` | Ethereum deposit, transition-verification, and withdrawal contract. |

## Verification paths

### Settlement

The settlement path verifies a Zeko/o1 Kimchi proof inside SP1. Ethereum then
checks that the extracted verification-key hash, action-state precondition, and
root transition match its stored state.

[Read the settlement flow →](/protocol/settlement)

### Deposit bridge

The deposit path replays an ordered range of Ethereum deposits, updates the
deposit accumulator, and computes the Zeko actions that represent those
deposits.

[Read the deposit bridge flow →](/protocol/deposit-bridge)

### Withdrawals

The withdrawal path replays Zeko withdrawal actions, computes an Ethereum
withdrawal accumulator and fixed-depth Merkle root, and permits compact Merkle
claims only after the transition is linked to consecutive settlement-recorded
action checkpoints.

[Read the withdrawal flow →](/protocol/withdrawals)
