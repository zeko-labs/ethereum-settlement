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
- A fresh gateway database rebuilds deposits, accepted bridge transitions,
  settlements, claims, the virtual outer account/action sequence, and settled
  withdrawal leaves from finalized Ethereum calldata/events plus canonical
  archive preimages; it does not require the original local proof jobs.
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
- `explorer-ui/` is a standalone React explorer for L2 blocks, transactions,
  accounts, SP1/Ethereum settlements, deposits, withdrawals, and canonical
  claims. The gateway joins a read-only OCaml archive view with its existing
  Ethereum and bridge indexes.
- Pending native withdrawal requests are classified in L2 transaction lists
  and detail pages before their next settlement, and browser activity survives
  reloads without relying on wallet/UI memory as protocol state.
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

This table is the retained full-zkVM audit checkpoint: all guests executed and
their public values were checked before submission. The operational gateway
now verifies settlement Pickles proofs natively with the same pinned verifier
and receipt derivation, so new settlement jobs normally store a null cycle
count. `API_EXECUTE_ONLY=true` still reproduces the full audit. The local
verifier accepted empty proof bytes only on Anvil chain ID 31337. No SP1 proof
or network request was generated.

## ERC-20 branch checkpoint

The ERC-20 port now has the proof and custody seam needed to turn a canonical
`submitDeposit` call into a Zeko witness action:

- Solidity registers an immutable Ethereum-token/Mina-token asset identity and
  deposit-capacity ceiling, takes exact ERC-20 custody, emits the canonical
  deposit fields, and protects the locked liability from emergency withdrawal.
- The gateway indexes finalized `BridgeDeposit` logs together with the
  immutable asset ID. The bridge guest verifies the V2 ERC-20 deposit leaf and
  converts those fields into the exact five-field outer Witness action using
  the asset-bound `Ethereum ERC20 deposit V1` Poseidon preimage.
- Zeko's hybrid Ethereum/custom-token circuit uses the same preimage and binds
  both 128-bit asset-ID limbs before an accepted deposit can move a bounded,
  pre-minted Mina Fungible Token inventory from the bridge vault to the user.
- Withdrawal settlement binds the exact inner action, asset ID, ERC-20 token,
  recipient, and UInt64 amount into a V3 leaf. Solidity releases only the
  matching token after the settlement delay, with per-token replay cursors and
  liabilities.
- The gateway and `@zeko-labs/eth-bridge-sdk` expose ERC-20 deposit status and
  delayed token-withdrawal claims. Solidity/Rust share the exact accumulator
  leaf schema, SP1/OCaml share the exact Poseidon action vector, and the
  mock-verifier contract test exercises the complete custody, checkpoint, and
  delayed-release sequence without requesting an SP1 proof.

This branch is not yet a deployable Mina Fungible Token product. The remaining
runtime work is to instantiate the asset-specific circuit in the sequencer and
prover protocol, deploy the unmodified standard token owner/admin plus the
proof-controlled vault and bounded inventory, and compose the returned bridge
proof with the standard owner's `approveBase` proof before submitting the L2
transaction. Each registered ERC-20 currently requires its own circuit/VK and
coordinated registry entry. Its immutable Solidity deposit cap must equal the
pre-minted inventory placed in that asset's L2 vault.

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
- Production ERC-20 token deployment, inventory governance, and standard-token
  owner proof composition.
- Bridge proof fees.
- A canonical Mina verification-key hash; the PoC uses SHA-256 of the exact
  verifier-index JSON bytes.
- Production governance, timelocks, and permissionless proof submission.

The old arbitrary-timeout/ERC20 deposit path and separate withdrawal guest are
retained for compatibility tests but are disabled by default. They are not the
native bridge protocol described in these docs.
