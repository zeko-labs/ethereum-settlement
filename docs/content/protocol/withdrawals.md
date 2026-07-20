# Native withdrawals

The current withdrawal path is settlement-bound and does not ask the user to
produce a Mina proof or an SP1 proof. The sequencer proves the L2 transition
once; users claim individual leaves with ordinary Keccak Merkle paths.

## L2 action and preimage

The user obtains, signs, and submits the normal Zeko native withdrawal
transaction. The OCaml inner bridge emits a three-field inner Witness action:

```text
[Witness = 0, withdrawal_aux, children_digest]
```

The archived clear preimage contains the Ethereum recipient and amount in
Zeko's 9-decimal native unit. The SP1 settlement guest recomputes the OCaml
`Withdrawal_params` Poseidon hash and requires it to equal the action aux.
Unknown or non-withdrawal inner actions remain in the ordered tree as raw,
non-claimable leaves, so the committed indices cannot be rearranged.

## Settlement-bound Keccak tree

For each action, SP1 first hashes the exact three fields. A native withdrawal
leaf binds:

```text
keccak256(
  "ZEKO_NATIVE_WITHDRAWAL_LEAF_V2",
  chain_id,
  bridge_address,
  global_action_index,
  recipient,
  zeko_amount,
  action_fields_hash
)
```

Leaves are placed in their original order in a depth-16 tree using the
`ZEKO_INNER_ACTION_NODE_V2` domain. The receipt contains the root, global start
index, and count. SP1 replays every Mina action and refuses to emit the root
unless the sequence reaches the Pickles-bound inner action state and length.

`ZekoSettlement` records this root only while accepting the matching V2
settlement. The bridge reads it directly from the settlement contract; no
administrator can submit a separate withdrawal root.

## Gateway discovery

Before settlement, the gateway reads canonical, applied inner actions from the
Zeko archive and exposes matching native withdrawal requests through:

- `GET /v1/bridge/withdrawal-requests?recipient=0x...&after=<global-index>`

This lets a browser resume immediately after reload without treating local
storage as authoritative.

After Ethereum consensus finality, the gateway exposes claimable leaves through:

- `GET /v1/bridge/withdrawals?recipient=0x...&after=<global-index>`
- `GET /v1/bridge/withdrawals/:sequence/:offset`

The response contains recipient, amount, action-fields hash, settlement
sequence, global index, 16 siblings, current virtual slot, claimable slot, and
live claim status.

The submitted proof job is not required for recovery. If the gateway database
is empty, it reads the accepted root/range from Ethereum and the ordered action
fields and withdrawal preimages from the recoverable Zeko archive. It stores
the leaves only after recomputing the entire fixed-depth root and matching the
Ethereum checkpoint exactly.

## Claim

The caller sends those values to `claimNativeWithdrawal`. The contract:

1. loads the root and slot upper bound from the accepted settlement
2. checks the offset and recomputes the global action index
3. requires `currentVirtualSlot >= commitSlotUpper + withdrawalDelaySlots`
4. verifies the fixed-depth Merkle path
5. enforces the recipient's monotonic `nextWithdrawalIndex`
6. converts the Zeko amount to wei by multiplying by 1 gwei
7. reduces native escrow liability and transfers ETH to the proof-bound
   recipient

Claims are permissionless: the transaction sender need not equal the recipient,
but funds always go to the address committed in the leaf.

The cursor permits a recipient to skip to a later global index. Doing so makes
any earlier withdrawal for that recipient unclaimable, matching the helper
account's monotonic processing model.

## Legacy path

`program/withdraw`, `submitWithdrawTransition`, and `claimWithdraw` remain for
older fixtures. New deployments leave `legacyWithdrawEnabled` false. The
separate withdrawal accumulator is not used by the native PoC described here.
