# Native and ERC-20 deposits

The PoC bridge accepts native ETH, converts finalized deposit logs into exact
Zeko outer Witness actions, and waits for a later real Zeko commit to
synchronize those actions. A bridge proof by itself does **not** finalize an L2
deposit.

## Fixed bridge identity

The Ethereum bridge proxy is represented inside the OCaml circuit as the
synthetic compressed key:

```text
x = uint160(EthereumZekoBridge proxy)
is_odd = false
```

That address is proof identity. Reserve the final CREATE2 proxy address before
building the OCaml bridge verification key and settlement guest. Changing the
proxy later requires rebuilding the OCaml circuit artifacts and SP1 programs.

## User deposit

The user calls the canonical `depositETH(zekoRecipient)` overload. The bridge:

- requires a nonzero value with 1 gwei granularity
- normalizes 18-decimal wei to Zeko's 9-decimal native unit
- fixes timeout and upper slot to `UInt32.max`
- appends a chain- and bridge-bound Keccak leaf to the deposit accumulator
- increments `depositNonce` and native escrow liability
- emits `BridgeDeposit`

The overload with a caller-selected timeout and the old arbitrary-token path is
disabled unless an administrator explicitly enables the legacy compatibility
switch. Canonically registered ERC-20 assets instead use `submitDeposit` and do
not depend on that switch.

## Canonical ERC-20 deposit

Each ERC-20 registry entry immutably binds the Ethereum token, standard Mina
FungibleToken owner, derived L2 token ID, decimals, capacity, chain, and bridge
proxy into an asset ID. `submitDeposit(token, amount, zekoRecipient)` requires
matching nine-or-fewer decimals on both chains, exact transfer custody, a
positive UInt64 amount, and remaining registered capacity. Its timeout is fixed
to `UInt32.max`.

The bridge guest verifies the V2 asset-bound deposit leaf and emits the same
five-field outer Witness shape as native deposits. Its auxiliary value is:

```text
Poseidon("Ethereum ERC20 deposit V1", [
  asset_id_high,
  asset_id_low,
  empty_call_forest,
  bridge_address_as_field,
  false,
  amount,
  recipient_x,
  recipient_is_odd,
  UInt32.max
])
```

The universal OCaml circuit authenticates the complete record against the
registry zkApp root/count, derives the token ID from the registered MFT owner,
and checks both asset-ID limbs before allowing the token-specific shared-vault
account to debit its pre-minted inventory. The browser SDK then places that
proved forest beneath the unmodified Mina Foundation FungibleToken owner's
`approveBase` proof. Different assets share the vault public key and bridge VK
but use distinct derived token IDs, balances, and replay-helper domains.

## Canonical proof input

After the deposit block is at or below Ethereum's consensus-finalized JSON-RPC
head, an operator calls `POST /v1/bridge/deposits/prove`. The gateway constructs
the batch itself from the next contiguous canonical finalized `BridgeDeposit`
rows. A caller cannot substitute deposit contents.

For each deposit the guest recomputes:

```text
deposit_leaf = keccak256(
  domain, chain_id, bridge_address, native_token,
  zeko_recipient, zeko_amount, UInt32.max, nonce
)

deposit_state_after = keccak256(
  state_domain, deposit_state_before, deposit_leaf
)
```

It also computes the OCaml-compatible auxiliary value:

```text
Poseidon("Ethereum deposit V1", [
  empty_call_forest,
  bridge_address_as_field,
  false,
  zeko_amount,
  recipient_x,
  recipient_is_odd,
  UInt32.max
])
```

and emits the exact five-field action:

```text
[Witness = 1, aux, children_digest = 0, slot_lower = 0, slot_upper = UInt32.max]
```

Each action includes its resulting Poseidon action-state checkpoint in the V2
bridge receipt.

## Ethereum acceptance

`EthereumZekoBridge.submitBridgeTransition` verifies the SP1 proof and binds
the receipt to:

- the bridge's proven deposit nonce and historical accumulator checkpoint
- the current on-chain deposit nonce and accumulator
- the settlement contract's current outer action state and length
- a nonempty, contiguous action range whose length equals the deposit count

The bridge then calls `appendOuterWitnessBatch` for every proof-emitted action.
This advances the settlement contract's outer action state and makes the exact
action bytes visible through the gateway's Mina `actions` query.

## Deposit synchronization

The sequencer reads those outer actions from the gateway. The next appropriate
OCaml commit must bind the final deposit action checkpoint as its synchronized
outer action state and length. Only after that settlement is confirmed does the
gateway mark the deposit synchronized.

The Ethereum gateway's Mina-compatible `actions` query returns finalized rows
only. The Ethereum sequencer profile therefore uses
`--deposit-delay-blocks 0`. Mina still uses the unchanged OCaml block-delay
filter in `update_inner_account_unlocked`; the adapter does not alter Mina's
behavior.

The user then obtains and signs the normal Zeko helper-account
`finalizeDeposit` update and submits it to the sequencer. The gateway never
forges that signature and the helper account remains responsible for its
processed-deposit cursor.

## User-facing status

`GET /v1/bridge/deposits/:nonce` reports Ethereum finality, bridge proof job,
exact outer action, synchronized settlement, and the next action:

```text
waitForEthereumFinality
requestBridgeProof
waitForSettlementSynchronization
finalizeDepositOnZeko
```

Cancellation is intentionally absent. Do not deposit funds into a PoC
deployment unless the operator is online and the lack of a refund path is
acceptable.
