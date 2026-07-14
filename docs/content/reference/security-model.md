# Security model

This is an experimental, permissioned testnet PoC. Its core proof and state
bindings are real, but its operational roles and multisig DA assumptions are
not a production decentralization model.

## Proof guarantees

An accepted settlement SP1 proof guarantees that:

- the configured Pickles verifier index accepted the OCaml proof, including
  accumulator, deferred-value, wrap-input, and outer Kimchi checks
- the account-update body and actions match the verified application statement
- the next outer state and action checkpoints were derived from the
  proof-bound commit
- a V2 inner-action tree replays to the proof-bound state/length and every
  clear native withdrawal matches its Poseidon aux

An accepted bridge SP1 proof guarantees that a supplied ordered deposit batch
produces its Keccak accumulator and exact Poseidon outer actions.

Proofs alone do not establish Ethereum log finality. The contracts bind public
values to on-chain accumulators and state; the gateway only builds the canonical
deposit input from finalized logs.

## Contract guarantees

`ZekoSettlement` enforces SP1 program vkey, Pickles VK identifier, chain and
contract domain, batch ordering, all eight source fields, outer action
state/length, known synchronization checkpoint, and virtual slot bounds.

`EthereumZekoBridge` enforces deposit accumulator continuity, contiguous proof
ranges, native escrow accounting, settlement-bound withdrawal roots, delay,
Merkle inclusion, and a monotonic per-recipient cursor.

The settlement proxy grants the bridge proxy `BRIDGE_ROLE`, allowing only that
contract to append bridge-proven outer Witness checkpoints.

## Operational trust

| Principal | Trust in the PoC |
| --- | --- |
| OCaml sequencer | Orders L2 transactions and decides when to commit, but cannot forge a valid Pickles transition. |
| 2-of-3 DA signers | Keep batch/checkpoint data available for the current milestone. This is not the production Ethereum DA design. |
| Gateway operator | Selects when to request proofs and submits transactions. Contract continuity limits forged state, but liveness and censorship remain permissioned. |
| `PROVER_ROLE` EOA | Can submit valid proofs, not alter admin configuration. |
| `ADMIN_ROLE` | Can change the PoC VK identifier, configure/enable legacy paths, pause, set bridge/delay, and emergency-withdraw custody. This is a strong trust assumption. |
| `UPGRADER_ROLE` | Can replace implementation logic and therefore has full protocol power. |
| `DEFAULT_ADMIN_ROLE` | Can grant/revoke roles. |

Keep default admin, admin, and upgrader keys off the runtime gateway. Production
would require deliberate governance/timelock and emergency-withdraw design.

## Artifact identity

Safety depends on deploying one coherent artifact set:

- final bridge proxy address embedded in OCaml circuits
- exact wrap verifier index embedded in the settlement guest
- exact SP1 ELFs represented by on-chain program vkeys
- matching genesis state/action checkpoint and virtual slot parameters
- matching three DA and sequencer identities

The gateway checks program vkeys at startup and the deployment preflight checks
the manifest, roles, addresses, chain ID, circuit holder, and DA identity. These
checks prevent accidental mixing; operators must still retain build provenance.

The PoC VK identifier is a SHA-256 file digest. A semantically identical JSON
file serialized differently has a different identity. Production must use the
canonical OCaml/Mina VK hash.

## Reorg and finality assumptions

Ethereum events affect the virtual Mina view only after the configured depth.
The gateway stores canonical block hashes and reversible account/action
snapshots. A reorg requeues the same paid proof rather than requesting another.

This protects cost and local consistency; it does not eliminate Ethereum
finality risk. Operators must halt on deep reorg or conflicting contract state.

## Known security limitations

- No deposit cancellation/refund path exists.
- Admin emergency withdrawal can violate user custody expectations.
- Multisig DA can withhold data.
- No blob payload/root binding or long-term blob archival exists.
- No ERC20 asset-ID/decimal protocol is specified.
- Legacy deposit/withdraw paths remain in storage-compatible code and must stay
  disabled.
- Public bridge discovery endpoints need rate limiting and abuse monitoring.
- The gateway GraphQL compatibility handler is not intended as a general
  Internet-facing Mina node.
- `LocalSP1Verifier` and empty proof bytes are safe only on local chain ID
  31337 and must never appear in a testnet manifest.
