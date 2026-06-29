# Deposit Bridge

The deposit bridge proves that an ordered range of Ethereum deposits produces
the expected Zeko deposit actions.

## Deposits on Ethereum

Users call `deposit` for ERC20 tokens or `depositETH` for native ETH on
`EthereumZekoBridge.sol`.

For each deposit, the contract:

1. Checks that the token is allowed and the amount is non-zero.
2. Locks the funds and rejects fee-on-transfer ERC20 tokens.
3. Normalizes the Ethereum amount to the configured Zeko decimals.
4. Increments `depositNonce`.
5. Computes a deposit leaf.
6. Appends the leaf to `currentDepositState`.
7. Stores the checkpoint in `depositStateByNonce`.

Packed Zeko addresses contain a Pasta Fp x-coordinate in the lower 255 bits and
the public-key parity flag in the highest bit.

## Proof input

`BridgeTransitionInput` contains:

| Field | Meaning |
| --- | --- |
| `ethereum.chain_id` | Chain ID included in every deposit leaf. |
| `ethereum.bridge_address` | Bridge address included in leaves and used as `holderAccountL1`. |
| `ethereum.deposit_nonce` | Nonce immediately before the batch. |
| `ethereum.deposit_state` | Deposit accumulator immediately before the batch. |
| `zeko.action_state` | Zeko action state immediately before the batch. |
| `deposits[]` | Ordered deposits to replay. |

Each deposit entry contains:

| Field | Meaning |
| --- | --- |
| `token` | Ethereum token address (zero = native ETH). |
| `amount` | Original Ethereum amount (informational, not hashed by the guest). |
| `zeko_amount` | Amount expressed in Zeko decimals. This is what gets hashed. |
| `zeko_recipient` | Packed Pasta Fp x-coordinate + parity bit in the high bit. |
| `timeout` | Deposit timeout slot included in the aux hash. |
| `children_digest` | Keccak/Poseidon hash of the zkapp call forest attached to this action on Mina. Constant for standard same-bridge transactions. |
| `slot_range_lower` | Mina slot range lower bound included in the outer witness action fields. |
| `slot_range_upper` | Mina slot range upper bound included in the outer witness action fields. |

## Deposit accumulator

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

## Zeko deposit action

The guest unpacks `zeko_recipient` into `(x, isOdd)` and computes an auxiliary
hash:

```text
aux = Poseidon.hashWithPrefix("Deposit_params - qFB3jXP*)", [
  Field(0),
  holderAccountL1,
  zekoAmount,
  recipient.x,
  recipient.isOdd,
  timeout
])
```

This aux is then placed into a 5-field outer witness action, which is the format
the Mina L1 bridge contract dispatches:

```text
action_fields = [1, aux, children_digest, slot_range_lower, slot_range_upper]
```

- `field[0] = 1` — discriminant identifying this as an outer witness (vs. 0 for commits)
- `field[1]` — aux hash above
- `field[2]` — `children_digest` from the input
- `field[3]` — `slot_range_lower`
- `field[4]` — `slot_range_upper`

Each deposit is wrapped in its own Mina action list and appended to the running
action state using the same domain-separated Poseidon operations as o1js:

```text
event_hash    = Poseidon.hashWithPrefix("MinaZkappEvent******", action_fields)
action_list   = Poseidon.hashWithPrefix("MinaZkappSeqEvents**", [empty, event_hash])
state_after   = Poseidon.hashWithPrefix("MinaZkappSeqEvents**", [state_before, action_list])
```

where `empty = Poseidon.emptyHashWithPrefix("MinaZkappActionsEmpty")`.

## Public values

| Field | Meaning |
| --- | --- |
| `ethereum_state_before` | Deposit accumulator before the batch. |
| `ethereum_state_after` | Deposit accumulator after the batch. |
| `ethereum_nonce_before` | Deposit nonce before the batch. |
| `ethereum_nonce_after` | Deposit nonce after the batch. |
| `zeko_action_state_before` | Supplied Zeko action state before the batch. |
| `zeko_action_state_after` | Computed Zeko action state after the batch. |
| `deposit_count` | Number of replayed deposits. |

## Contract checks

`submitBridgeTransition` verifies:

```text
depositStateByNonce[ethereum_nonce_before] == ethereum_state_before
ethereum_nonce_after                       == depositNonce
ethereum_state_after                       == currentDepositState
ethereum_nonce_after                       == ethereum_nonce_before + deposit_count
zeko_action_state_after                    has not already been processed
```

This binds the proven batch to the current Ethereum deposit history.

::: warning Settlement binding
The deposit transition currently does not require its before or after Zeko
action state to be a checkpoint recorded by `ZekoSettlement`. The accepted
event alone does not prove that Zeko consumed the deposit actions.
:::
