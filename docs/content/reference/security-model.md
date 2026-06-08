# Security Model

The proofs and contracts deliberately verify different parts of the system.

## Proof guarantees

- Settlement proves Kimchi validity and extracts a Zeko state transition.
- A deposit bridge proof proves the deterministic transformation from supplied
  deposits to an Ethereum accumulator and Zeko action state.
- A withdrawal proof proves the deterministic transformation from supplied
  withdrawals to an accumulator and Zeko action state.

SP1 verification does not prove that arbitrary off-chain inputs originated
from Ethereum or Zeko. Contract-side continuity checks provide that binding.

## Contract guarantees

`ZekoSettlement.sol` binds settlement outputs to Ethereum's stored root,
verification-key hash, configured action state, and SP1 program verification
key.

`EthereumZekoBridge.sol` binds deposit transitions to the deposit accumulator
recorded on Ethereum. It accepts withdrawal transitions only between
consecutive settlement-recorded action checkpoints.

## Administrative trust

Administrative roles can:

- change settlement verification-key and action-state parameters
- configure bridge tokens
- pause the bridge
- perform emergency withdrawals
- authorize implementation upgrades

Both contracts are UUPS implementations and must be deployed behind
`ERC1967Proxy` proxies. `PROVER_ROLE` can submit proofs, while `ADMIN_ROLE` and
`UPGRADER_ROLE` control administration and upgrades.

::: danger Deposit action-state limitation
Deposit transition acceptance currently does not require a settlement-recorded
Zeko action-state checkpoint. Consumers must not interpret a
`BridgeTransitionAccepted` event alone as proof that Zeko consumed the actions.
:::
