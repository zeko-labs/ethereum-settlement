# Current status

The multisig-DA PoC implementation is complete enough to run the full native
bridge round trip locally with real OCaml-produced commits. It has not yet been
promoted to a persistent live Sepolia deployment.

## Working today

- The OCaml committer exports the real wrap proof and verifier index, full
  Pickles statement skeleton, zkApp statement, account-update body preimage,
  actions, source outer state, and archived native-withdrawal preimages.
- SP1 performs accumulator checking, deferred-value reconstruction, wrap
  public-input reconstruction, and outer Kimchi verification through the o1
  Pickles verifier path.
- Settlement receipts bind all eight outer-state fields, outer action state and
  length, synchronized outer checkpoint, slot range, Ethereum domain, batch
  number, and verification-key identifier.
- The V2 settlement receipt binds exact inner actions to a depth-16 Keccak tree.
- Finalized native ETH deposit logs are converted into exact five-field outer
  Witness actions by the bridge guest.
- Solidity verifies state continuity, records synchronized checkpoints,
  enforces withdrawal delay, and releases native ETH with an ordinary Merkle
  proof.
- The gateway implements the sequencer's Mina GraphQL subset, persists proof
  jobs, gates paid requests on approval, waits for consensus finality, and rolls its
  virtual Mina view back on Ethereum reorgs.
- Public browser APIs expose deployment config, resumable deposit state, and
  precision-safe withdrawal proofs. Finalized deposits can be queued
  automatically without an operator API call.
- `@zeko-labs/eth-bridge-sdk` owns Ethereum wallet operations and composes the
  existing bridge SDK for sequencer-side deposit finalization and withdrawal
  requests. The Actions services consume the gateway's Mina archive shape.
- `bridge-ui/` is a standalone React application for the four browser-owned
  operations: ETH deposit, Zeko deposit finalization, Zeko withdrawal request,
  and Ethereum claim. It uses injected Ethereum wallets and Auro, with the PoC
  signing domain fixed to Mina `testnet`.
- The machine-local delivery workflow generates separated retained identities,
  builds digest-pinned gateway/Zeko/DA images, deploys only behind an explicit
  Sepolia deployment confirmation, materializes runtime files, and preflights the complete
  contract role/vkey/source identity.

## Verified local checkpoint

The July 15, 2026 retained-identity round trip used a genuine two-commit OCaml
export with DA quorum 2 of 3 and Mina signing domain `testnet`. Its verifier
index SHA-256 is
`a9bca935bad09638d2e335a7cfc4ecc110389d15c033db6b0413593ca9193c74`.
It locked 10 ETH and claimed 5 ETH after the deposit-synchronizing and
withdrawal-bearing settlements. The browser-readiness path submitted the
deposit and claim through `@zeko-labs/eth-bridge-sdk`, automatically queued the
finalized deposit, and observed its outer Witness through the production
Actions indexer/API pair.

The live-sequencer checkpoint additionally runs that browser SDK against the
real OCaml bridge GraphQL mutations. Deposit finalization is prepared from the
Actions API, signed by the browser client, accepted and proved by the OCaml
sequencer, and followed by an Ethereum-routed native withdrawal request. The
resulting deposit-synchronizing and withdrawal-bearing commits are exported for
settlement validation; this checkpoint generates no SP1 proof.

| Execution | SP1 cycles | Ethereum gas in local mock submission |
| --- | ---: | ---: |
| Native deposit bridge | 3,435,291 | 218,336 |
| Deposit-synchronizing settlement | 52,191,513,620 | 275,825 |
| Withdrawal-bearing settlement | 52,187,890,758 | 318,602 |

All guests executed and their public values were checked before submission.
The local verifier accepted empty proof bytes only on Anvil chain ID 31337. No
SP1 proof or network request was generated.

## Needed for the live PoC

1. Build and record immutable machine-local images from the final committed
   source and retained verifier index.
2. Provide a funded Sepolia RPC/admin/gateway identity and a funded Succinct
   requester, deploy the settlement and bridge proxies, and pass preflight.
3. Obtain a network-simulation PGU value for each genuine job, review the
   capped quote, and explicitly approve the three paid proofs used by the demo.
4. Complete one browser-driven Sepolia round trip and archive transaction,
   proof-request, cost, confirmation, DA, and balance/liability evidence.

No Sepolia deployment or paid Succinct proof has been performed by the checked
in delivery workflow yet. Those remain credential-, funding-, and
approval-gated external operations.

## Explicitly out of scope

- EIP-4844 blob DA, blob archival, and blob-to-Zeko data-root equivalence.
- Deposit cancellation and refunds.
- ERC20 deposit/withdrawal semantics.
- Bridge proof fees.
- A canonical Mina verification-key hash; the PoC uses SHA-256 of the exact
  verifier-index JSON bytes.
- Production governance, timelocks, and permissionless proof submission.

The old arbitrary-timeout/ERC20 deposit path and separate withdrawal guest are
retained for compatibility tests but are disabled by default. They are not the
native bridge protocol described in these docs.
