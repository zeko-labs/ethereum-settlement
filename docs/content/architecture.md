# Architecture

The system separates expensive proof computation from Ethereum-side state
continuity checks.

![Zeko Ethereum L2 architecture showing settlement, deposit bridge, and withdrawal flows](/architecture-flow.png)

## SP1 programs

Each guest program commits a narrowly scoped public output:

- Settlement commits proof validity and the extracted Zeko state transition.
- Bridge commits Ethereum deposit accumulator and Zeko action-state transitions.
- Withdraw commits Zeko action-state, Ethereum sequential withdrawal
  accumulator, and fixed-depth withdrawal Merkle root transitions.

The host binaries in `script/src/bin` parse fixtures, prepare SP1 inputs,
execute or prove the guest programs, and decode their public values. Shared
types live in `lib/src/lib.rs`.

## Ethereum contracts

`ZekoSettlement.sol` verifies settlement proofs, tracks the current root, and
records accepted action-state checkpoints.

`EthereumZekoBridge.sol`:

- locks ETH and ERC20 deposits
- maintains an append-only deposit accumulator
- verifies deposit and withdrawal SP1 transitions
- accepts withdrawal accumulator checkpoints
- validates withdrawal claims and releases locked assets

Both contracts are UUPS implementations intended to run behind
`ERC1967Proxy` proxies.

## Roles

| Role | Responsibility |
| --- | --- |
| `DEFAULT_ADMIN_ROLE` | Grants and revokes roles. |
| `ADMIN_ROLE` | Manages settlement parameters, tokens, pause state, and emergency withdrawals. |
| `PROVER_ROLE` | Submits SP1 proofs and transition public values. |
| `UPGRADER_ROLE` | Authorizes UUPS implementation upgrades. |

See the [security model](/reference/security-model) for the exact trust
boundaries between proofs, contracts, and administrative roles.
