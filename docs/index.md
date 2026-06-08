---
layout: default
title: Zeko Ethereum L2
---

# Zeko Ethereum L2

This repository uses SP1 proofs to connect Zeko state transitions with Ethereum
contracts. It contains three separate SP1 programs:

| Program | Direction | Purpose |
| --- | --- | --- |
| `program/settlement` | Zeko to Ethereum | Verifies a Zeko/o1 zkApp proof and exposes the rollup root transition. |
| `program/bridge` | Ethereum to Zeko | Replays an ordered batch of Ethereum deposits and computes the matching Zeko action-state transition. |
| `program/withdraw` | Zeko to Ethereum | Replays an ordered batch of Zeko withdrawal actions and computes the matching Ethereum withdrawal accumulator. |

The SP1 programs perform expensive proof verification and hashing off-chain.
The Ethereum contracts verify the resulting succinct SP1 proofs and enforce
continuity against state already stored on Ethereum.

## Architecture

```text
                         Zeko zkApp proof
                                |
                                v
                     settlement SP1 program
                                |
                                v
                       ZekoSettlement.sol
                      root + action checkpoints
                                |
                                v
Ethereum deposits --> bridge SP1 program ----\
                                              > EthereumZekoBridge.sol
Zeko withdrawals -> withdraw SP1 program ----/  deposits + withdrawals
```

The host binaries in `script/src/bin` parse fixtures, prepare SP1 inputs,
execute or prove the guest programs, and decode their public values. Shared
input and output types live in `lib/src/lib.rs`.

## Settlement Flow

Settlement proves that a specific Zeko/o1 zkApp proof is valid and exposes the
state transition encoded by the first account update.

### 1. Host preparation

The settlement host binary reads:

- a base64-encoded Zeko verification key from `proofs/vk.txt`
- a GraphQL zkApp command and proof from `proofs/graphql.txt`

The host derives the zkApp statement, computes deferred proof values, builds
the verifier index, and serializes all required inputs for the SP1 guest.

### 2. Settlement guest verification

`program/settlement` performs the following work inside SP1:

1. Deserializes the verification key, o1 proof, zkApp statement, deferred
   values, zkApp command, and verifier index.
2. Binds the supplied statement to the first account update by checking both
   the account-update digest and calls hash.
3. Loads the embedded Pasta SRS from `srs_rkyv.bin`.
4. Restores verifier-index fields omitted from serialization, including
   linearization data, powers of alpha, and the endomorphism constant.
5. Checks selected verifier-index commitments against the supplied Zeko
   verification key.
6. Reconstructs the Kimchi public inputs and verifies the o1 proof.
7. Extracts the first account update's app-state preconditions, app-state
   updates, and action-state precondition.
8. Commits the result as SP1 public values.

The guest aborts if Kimchi verification fails. Therefore, a successfully
verified SP1 proof always contains `proof_valid = true`.

### 3. Settlement public values

`ZkappPublicValues` is serialized in the following order:

| Field | Meaning |
| --- | --- |
| `proof_valid` | Whether the Kimchi proof verified. |
| `vk_hash` | Hash of the supplied Zeko verification key. |
| `state_before[8]` | Checked app-state preconditions from the first account update. Ignored slots become zero. |
| `state_after[8]` | Explicit app-state updates from the first account update. Kept slots become zero. |
| `action_state_before` | Checked action-state precondition. An ignored precondition becomes zero. |

Only app-state slot `3` is currently interpreted as the rollup root by the
Ethereum contract. Because `Keep` and ignored values are emitted as zero, the
settled transition must explicitly constrain `state_before[3]` and explicitly
set `state_after[3]`.

### 4. Ethereum settlement checks

`contracts/src/ZekoSettlement.sol` first asks the configured SP1 verifier to
verify the proof under `programVKey`. It then checks:

```text
publicValues.proof_valid           == true
publicValues.vk_hash               == vkHash
publicValues.action_state_before   == actionState
publicValues.state_before[3]       == currentRoot
```

If all checks pass:

```text
currentRoot = publicValues.state_after[3]
```

The contract also records the accepted `action_state_before` as an indexed L2
action-state checkpoint. These checkpoints are later used to authorize
withdraw transitions.

Important: settlement does not derive or advance `actionState` from the proof.
An account with `ADMIN_ROLE` updates it through `setActionState`. The value is
used as a guard for future settlement proofs and as a checkpoint source for
withdraw verification.

## Deposit Bridge Flow

The deposit bridge proves that an ordered range of deposits from the
Ethereum-side accumulator produces the expected Zeko deposit actions.

### 1. Deposits on Ethereum

Users call `deposit` for ERC20 tokens or `depositETH` for native ETH on
`EthereumZekoBridge.sol`.

For each deposit, the contract:

1. Checks that the token is allowed and the amount is non-zero.
2. Locks the funds. Fee-on-transfer ERC20 tokens are rejected.
3. Normalizes the Ethereum amount to the configured Zeko decimals.
4. Increments `depositNonce`.
5. Computes a deposit leaf.
6. Appends the leaf to `currentDepositState`.
7. Stores the accumulator checkpoint in `depositStateByNonce`.

The contract validates that packed Zeko addresses contain a valid Pasta Fp
x-coordinate. The highest bit stores the public-key parity flag.

### 2. Bridge proof input

`BridgeTransitionInput` contains:

| Field | Meaning |
| --- | --- |
| `ethereum.chain_id` | Chain ID included in every deposit leaf. |
| `ethereum.bridge_address` | Bridge address included in leaves and used as the Zeko `holderAccountL1`. |
| `ethereum.deposit_nonce` | Nonce immediately before the batch. |
| `ethereum.deposit_state` | Deposit accumulator immediately before the batch. |
| `zeko.action_state` | Zeko action state immediately before the batch. |
| `deposits[]` | Ordered deposits to replay. |

Each `BridgeDeposit` contains `token`, `amount`, `zeko_amount`,
`zeko_recipient`, and `timeout`. The guest uses `zeko_amount`; the original
Ethereum `amount` is informational and is not included in the proof
calculation.

The prover must construct the batch from the same ordered deposit data emitted
by the Ethereum contract. A proof cannot change the final on-chain deposit
accumulator because the contract checks its final nonce and state.

### 3. Deposit leaf and accumulator

For each deposit, the guest increments the nonce and computes values equivalent
to Solidity `keccak256(abi.encode(...))`:

```text
deposit_leaf = keccak256(
  keccak256("ZEKO_BRIDGE_DEPOSIT_LEAF_V1"),
  chain_id,
  bridge_address,
  token,
  zeko_recipient,
  zeko_amount,
  timeout,
  nonce
)

deposit_state_after = keccak256(
  keccak256("ZEKO_BRIDGE_DEPOSIT_STATE_V1"),
  deposit_state_before,
  deposit_leaf
)
```

### 4. Zeko deposit action

The guest unpacks `zeko_recipient` into `(x, isOdd)` and computes:

```text
action = Poseidon.hashWithPrefix("Deposit_params - qFB3jXP*)", [
  Field(0),
  holderAccountL1,
  zekoAmount,
  recipient.x,
  recipient.isOdd,
  timeout
])
```

Each deposit is wrapped in its own Mina action list and appended to the action
state with the same domain-separated Poseidon operations used by o1js:

```text
event_hash       = hashWithPrefix("MinaZkappEvent******", [action])
action_list_hash = hashWithPrefix("MinaZkappSeqEvents**", [empty_list, event_hash])
action_state     = hashWithPrefix("MinaZkappSeqEvents**", [action_state, action_list_hash])
```

### 5. Bridge public values and contract checks

`BridgeTransitionPublicValues` contains:

| Field | Meaning |
| --- | --- |
| `ethereum_state_before` | Deposit accumulator before the batch. |
| `ethereum_state_after` | Deposit accumulator after the batch. |
| `ethereum_nonce_before` | Deposit nonce before the batch. |
| `ethereum_nonce_after` | Deposit nonce after the batch. |
| `zeko_action_state_before` | Supplied Zeko action state before the batch. |
| `zeko_action_state_after` | Computed Zeko action state after the batch. |
| `deposit_count` | Number of replayed deposits. |

`submitBridgeTransition` verifies the SP1 proof and enforces:

```text
depositStateByNonce[ethereum_nonce_before] == ethereum_state_before
ethereum_nonce_after                       == depositNonce
ethereum_state_after                       == currentDepositState
ethereum_nonce_after                       == ethereum_nonce_before + deposit_count
zeko_action_state_after                    has not already been processed
```

This binds the proven deposit batch to the current Ethereum deposit history.
The contract emits `BridgeTransitionAccepted` and marks the final Zeko action
state as processed.

The deposit transition currently does **not** require its before or after Zeko
action state to be a checkpoint recorded by `ZekoSettlement`. Consumers must
not interpret the event alone as proof that Zeko accepted the actions.

## Withdrawal Flow

The withdrawal program proves that an ordered batch of Zeko withdrawal actions
produces a specific Ethereum withdrawal accumulator.

### 1. Withdraw proof calculation

`WithdrawTransitionInput` contains the chain ID, bridge address, current
withdraw accumulator, starting Zeko action state, and an ordered list of
withdrawals.

Each withdrawal contains:

| Field | Meaning |
| --- | --- |
| `token` | Zeko field encoding an Ethereum token address in its low 160 bits. Zero means native ETH. |
| `recipient` | Zeko field encoding the Ethereum recipient in its low 160 bits. |
| `amount` | Amount expressed using the token's configured Zeko decimals. |

For every withdrawal, the guest computes:

```text
withdraw_leaf = keccak256(
  keccak256("ZEKO_BRIDGE_WITHDRAW_LEAF_V1"),
  chain_id,
  bridge_address,
  token,
  recipient,
  amount
)

withdraw_state_after = keccak256(
  keccak256("ZEKO_BRIDGE_WITHDRAW_STATE_V1"),
  withdraw_state_before,
  withdraw_leaf
)
```

It also computes and appends the matching Zeko action:

```text
action = Poseidon.hashWithPrefix("Withdrawal_params - qFB3jXP*)", [
  Field(0),
  amount,
  recipient
])
```

### 2. Accepting a withdraw transition

`WithdrawTransitionPublicValues` contains:

| Field | Meaning |
| --- | --- |
| `zeko_action_state_before` | Zeko action state before the batch. |
| `zeko_action_state_after` | Zeko action state after the batch. |
| `ethereum_withdraw_state_before` | Ethereum withdrawal accumulator before the batch. |
| `ethereum_withdraw_state_after` | Ethereum withdrawal accumulator after the batch. |
| `withdraw_count` | Number of withdrawals in the batch. |

`submitWithdrawTransition` verifies the SP1 proof and requires:

- the starting withdrawal accumulator equals `currentWithdrawState`
- the final action state has not already been processed
- both action states are checkpoints recorded by `ZekoSettlement`
- the old checkpoint matches `currentWithdrawActionStateIndex`
- the new checkpoint index is exactly the old index plus one

For a non-empty batch, the final withdrawal accumulator becomes a valid claim
state. The bridge records the old action-state index used to scope withdrawal
nullifiers, then advances `currentWithdrawState` and
`currentWithdrawActionStateIndex`.

### 3. Claiming a withdrawal

To claim, a caller supplies:

- the accumulator state before the batch
- an accepted accumulator state after the batch
- the clear withdrawal being claimed
- its index in the batch
- the full ordered list of withdrawal leaf hashes

The contract replaces the hash at the claimed index with the leaf recomputed
from the clear withdrawal, replays the complete accumulator sequence, and
requires the result to equal the accepted final state.

It then computes a nullifier from the old action-state index, withdrawal index,
and leaf. A spent nullifier cannot be claimed again. Finally, the contract
validates the token and recipient field encodings, converts the Zeko amount
back to Ethereum decimals, and transfers the locked ETH or ERC20 tokens.

## Trust Boundaries

The proofs and contracts deliberately prove different parts of the system:

- Settlement proves Kimchi validity and binds the extracted root transition to
  Ethereum's stored root, verification-key hash, and configured action state.
- A deposit bridge proof proves the deterministic transformation from supplied
  deposits to an Ethereum accumulator and Zeko action state. The Ethereum
  contract binds the final accumulator to deposits recorded on-chain.
- A withdraw proof proves the deterministic transformation from supplied
  withdrawals to an accumulator and Zeko action state. The Ethereum contract
  accepts it only between consecutive settlement-recorded action checkpoints.
- SP1 verification does not prove that an arbitrary off-chain input originated
  from Ethereum or Zeko. Contract-side continuity checks provide that binding.
- Administrative roles can change settlement parameters, token availability,
  pause the bridge, perform emergency withdrawals, and authorize upgrades.

Both contracts are UUPS implementations and must be deployed behind
`ERC1967Proxy` proxies. `PROVER_ROLE` can submit proofs, while `ADMIN_ROLE` and
`UPGRADER_ROLE` control administration and upgrades.

## Commands

Execute the programs without generating proofs:

```sh
cargo run --release --bin zkapp -- --execute
cargo run --release --bin bridge -- --execute
cargo run --release --bin withdraw -- --execute
```

Use larger bridge fixtures:

```sh
cargo run --release --bin bridge -- --execute --input proofs/bridge-input-200.json
cargo run --release --bin withdraw -- --execute --input proofs/withdraw-input-200.json
```

Generate local core proofs:

```sh
cargo run --release --bin bridge -- --prove
cargo run --release --bin withdraw -- --prove
```

Run regression and contract tests:

```sh
cargo test -p bridge-program fixture_deposit_matches_zeko_action_state
cargo test -p withdraw-program
cd contracts && forge test
```

The o1js fixture reproduces the deposit action-state update:

```sh
cd tools/zeko-action-state
npm install
npm start
```

## Fixture Checkpoint

`proofs/bridge-input.json` contains three deposits. Its expected action-state
transition is:

```text
before: 0x3772bc5435b957f81f86f752e93f2e29e886ac24580b3d1ec879c1dad26965f9
after : 0x3d638b908c4241e7b417d1790a79d0fe3277a133a5a87e12a484cd756de795bf
```
