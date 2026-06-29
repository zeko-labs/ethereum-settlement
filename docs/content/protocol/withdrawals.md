# Withdrawals

The withdrawal program proves that an ordered batch of Zeko withdrawal actions
produces a fixed-depth withdrawal Merkle root and the corresponding Ethereum
withdrawal state transition.

## Proof input

`WithdrawTransitionInput` contains the chain ID, bridge address, current
withdrawal state, starting Zeko action state, and an ordered withdrawal list.

| Field | Meaning |
| --- | --- |
| `token` | Zeko field encoding an Ethereum token address in its low 160 bits. Zero means native ETH. |
| `recipient` | Zeko field encoding the Ethereum recipient in its low 160 bits. |
| `amount` | Amount expressed using the token's configured Zeko decimals. |
| `children_digest` | Poseidon hash of the zkapp call forest attached to this withdrawal action on Zeko. Constant for standard same-bridge transactions. |

Zeko currently supports only the native token in withdrawal actions. The SP1
program rejects every withdrawal whose `token` field is not zero.

## Withdraw accumulator

For every withdrawal, the guest computes a withdrawal leaf:

```text
withdraw_leaf = keccak256(
  keccak256("ZEKO_BRIDGE_WITHDRAW_LEAF_V1"),
  chain_id,
  bridge_address,
  token,
  recipient,
  amount
)
```

It also computes and appends the matching Zeko action. First, an auxiliary hash
is derived from the withdrawal fields:

```text
aux = Poseidon.hashWithPrefix("Withdrawal_params - qFB3jXP*)", [
  Field(0),
  amount,
  recipient
])
```

This aux is then placed into a 3-field inner action, which is the format the
Zeko L2 bridge account dispatches:

```text
action_fields = [0, aux, children_digest]
```

- `field[0] = 0` — discriminant identifying this as a withdrawal commit (vs. 1 for outer witness)
- `field[1]` — aux hash above
- `field[2]` — `children_digest` from the input

Each withdrawal is wrapped in its own Mina action list and appended to the
running action state using the same domain-separated Poseidon operations as
o1js:

```text
empty         = Poseidon.emptyHashWithPrefix("MinaZkappActionsEmpty")
event_hash    = Poseidon.hashWithPrefix("MinaZkappEvent******", action_fields)
action_list   = Poseidon.hashWithPrefix("MinaZkappSeqEvents**", [empty, event_hash])
state_after   = Poseidon.hashWithPrefix("MinaZkappSeqEvents**", [state_before, action_list])
```

The same ordered withdrawal leaves are committed into a depth-16 Keccak Merkle
tree. The tree supports at most 65,536 withdrawals and pads unused leaves with
`bytes32(0)`.

```text
node = keccak256(
  keccak256("ZEKO_BRIDGE_WITHDRAW_MERKLE_NODE_V1"),
  left,
  right
)
```

After building the root, the guest computes the next withdrawal state once for
the complete batch:

```text
withdraw_state_after = keccak256(
  keccak256("ZEKO_BRIDGE_WITHDRAW_STATE_V1"),
  withdraw_state_before,
  withdrawal_root,
  withdraw_count
)
```

The withdrawal count is included because unused tree leaves are padded with
`bytes32(0)`.

## Public values

| Field | Meaning |
| --- | --- |
| `zeko_action_state_before` | Zeko action state before the batch. |
| `zeko_action_state_after` | Zeko action state after the batch. |
| `ethereum_withdraw_state_before` | Ethereum withdrawal accumulator before the batch. |
| `ethereum_withdraw_state_after` | Ethereum withdrawal accumulator after the batch. |
| `withdrawal_root` | Depth-16 Merkle root over the same ordered withdrawal leaves. |
| `withdraw_count` | Number of withdrawals in the batch. |

## Accepting a transition

`submitWithdrawTransition` verifies the SP1 proof and requires:

- the starting withdrawal state equals `currentWithdrawState`
- the final withdrawal state equals the V1 hash of the starting state, root,
  and withdrawal count
- the final action state has not already been processed
- both action states are checkpoints recorded by `ZekoSettlement`
- the old checkpoint matches `currentWithdrawActionStateIndex`
- the new checkpoint index is exactly the old index plus one

For a non-empty batch, the final withdrawal accumulator becomes a valid claim
transition and the Merkle root becomes a valid claim root. The bridge stores
one withdrawal batch record under the old Zeko action state bound by the SP1
proof. That record contains the Merkle root, withdrawal states, checkpoint
index, and withdrawal count. The same Merkle root may safely appear in
different action-state transitions.

## Claiming a withdrawal

To claim, a caller supplies:

- the old Zeko action state bound to the withdrawal batch
- the clear withdrawal being claimed
- its index in the batch
- a fixed 16-sibling Merkle proof

The contract recomputes the leaf and verifies its Merkle proof against the
root stored for that old action state. Claims no longer require the root or the
full ordered withdrawal batch in calldata.

It then computes a nullifier from the old action-state index, withdrawal index,
and leaf. A spent nullifier cannot be claimed again. Finally, the contract
validates token and recipient field encodings, converts the Zeko amount back to
Ethereum decimals, and transfers the locked ETH or ERC20 tokens.
