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
  jobs, gates paid requests on approval, waits for confirmations, and rolls its
  virtual Mina view back on Ethereum reorgs.
- Public browser APIs expose deployment config, resumable deposit state, and
  precision-safe withdrawal proofs. Finalized deposits can be queued
  automatically without an operator API call.
- `@zeko-labs/eth-bridge-sdk` owns Ethereum wallet operations and composes the
  existing bridge SDK for sequencer-side deposit finalization and withdrawal
  requests. The Actions services consume the gateway's Mina archive shape.

## Verified local checkpoint

The July 14, 2026 round trip used a real two-commit OCaml export with DA quorum
2 of 3. It locked 10 ETH and claimed 5 ETH after the deposit-synchronizing and
withdrawal-bearing settlements. The July browser-readiness rerun submitted the
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
| Native deposit bridge | 3,433,016 | 218,288 |
| Deposit-synchronizing settlement | 52,186,638,600 | 275,837 |
| Withdrawal-bearing settlement | 52,189,369,576 | 318,590 |

All guests executed and their public values were checked before submission.
The local verifier accepted empty proof bytes only on Anvil chain ID 31337. No
SP1 proof or network request was generated.

## Needed for the live PoC

1. Retain a stable testnet genesis, sequencer/DA identities, exact circuit
   configuration, OCaml verifier index, and bridge address.
2. Build and pin Zeko and gateway images from that identity.
3. Deploy the real SP1 verifier plus the settlement and bridge proxies on
   Sepolia, then assign separated roles and fund the required accounts.
4. Add the gateway and Ethereum-specific sequencer settings to the NixOS
   machines repository, or deploy the pinned Compose profile on a dedicated
   operator host.
5. Run each genuine job through execute-only validation, approve its quoted
   cost, obtain the three network proofs used by the demo, and complete the
   acceptance checklist.

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
