# Withdrawals

The withdrawal program proves that an ordered batch of Zeko withdrawal actions
produces a specific Ethereum withdrawal accumulator.

## Proof input

`WithdrawTransitionInput` contains the chain ID, bridge address, current
withdraw accumulator, starting Zeko action state, and an ordered withdrawal
list.

| Field | Meaning |
| --- | --- |
| `token` | Zeko field encoding an Ethereum token address in its low 160 bits. Zero means native ETH. |
| `recipient` | Zeko field encoding the Ethereum recipient in its low 160 bits. |
| `amount` | Amount expressed using the token's configured Zeko decimals. |

## Withdraw accumulator

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

## Public values

| Field | Meaning |
| --- | --- |
| `zeko_action_state_before` | Zeko action state before the batch. |
| `zeko_action_state_after` | Zeko action state after the batch. |
| `ethereum_withdraw_state_before` | Ethereum withdrawal accumulator before the batch. |
| `ethereum_withdraw_state_after` | Ethereum withdrawal accumulator after the batch. |
| `withdraw_count` | Number of withdrawals in the batch. |

## Accepting a transition

`submitWithdrawTransition` verifies the SP1 proof and requires:

- the starting withdrawal accumulator equals `currentWithdrawState`
- the final action state has not already been processed
- both action states are checkpoints recorded by `ZekoSettlement`
- the old checkpoint matches `currentWithdrawActionStateIndex`
- the new checkpoint index is exactly the old index plus one

For a non-empty batch, the final withdrawal accumulator becomes a valid claim
state. The bridge records the old action-state index used to scope withdrawal
nullifiers and advances its current withdrawal state.

## Claiming a withdrawal

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
validates token and recipient field encodings, converts the Zeko amount back to
Ethereum decimals, and transfers the locked ETH or ERC20 tokens.
