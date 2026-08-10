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
- The gateway indexes finalized `BridgeDeposit` and immutable ERC-20 identity
  events. Legacy one-token deposits remain encoding V1 with the
  `ZEKO_ERC20_DEPOSIT_LEAF_V2`/`Ethereum ERC20 deposit V1` wire. Universal
  registry deposits use encoding V2 and bind the registry index plus canonical
  Mina record commitment in the `ZEKO_ERC20_DEPOSIT_LEAF_V3` Keccak leaf and
  `Ethereum ERC20 deposit V2` Poseidon preimage.
- Zeko's hybrid Ethereum/custom-token circuit authenticates that record,
  registry index, and both 128-bit asset-ID limbs before an accepted deposit
  can move a bounded, pre-minted Mina Fungible Token inventory from the bridge
  vault to the user.
- Withdrawal settlement retains the V3 leaf for legacy encoding V1 and uses a
  V4 leaf for registry encoding V2, binding the registry index and record
  commitment alongside the exact inner action, asset ID, ERC-20 token,
  recipient, and UInt64 amount. Solidity releases only the matching token
  after the settlement delay, with per-token replay cursors and liabilities.
- The gateway and `@zeko-labs/eth-bridge-sdk` expose ERC-20 deposit status and
  delayed token-withdrawal claims. Solidity/Rust share the exact accumulator
  leaf schema, SP1/OCaml share the exact Poseidon action vector, and the
  mock-verifier contract test exercises the complete custody, checkpoint, and
  delayed-release sequence without requesting an SP1 proof.

The universal registry runtime path is also implemented:

- The sequencer accepts one registry account, schema version, approved MFT
  standard VK ID, shared vault public key, and universal bridge VK ID. Asset
  records and depth-8 membership witnesses are dynamic circuit inputs. The
  current schema supports 256 records with at most nine decimals; adding a
  token does not compile another circuit or VK.
- Registration is an append-only Poseidon Merkle-list transition. Its recursive
  scan proves dense ordered traversal of every existing leaf and rejects
  duplicate Ethereum tokens, asset IDs, or L2 owner/token identities.
- Each outer Pickles commit binds the registry public key, root, count, and
  schema through a domain-separated Poseidon digest in the signed sequencer
  child's call data. This keeps the checkpoint in the verified call forest
  without requiring a shadow registry account on Mina L1.
- Solidity proposals remain `Pending` until a V4 settlement binds the exact
  ordered `(record hash, Mina record commitment)` batch and the new L2 registry
  root/count. Depth-8 Keccak batch proofs activate the corresponding immutable
  records without requiring Solidity to evaluate Poseidon.
- Registry selectors execute in a dedicated immutable module against
  namespaced proxy storage. The bridge retains custody configuration behind
  self-only callbacks; deterministic deployment records the module address and
  keeps both implementations below Ethereum's EIP-170 bytecode limit.
- The browser SDK validates the returned vault forest and exact full token-owner
  account-update body, then composes and proves the transaction with the
  unmodified `mina-fungible-token` owner's `approveBase` method.
- The Actions indexer reconstructs immutable records and current membership
  paths. The SDK resolves by Ethereum token, asset ID, or stable registry index
  and rejects malformed records, roots, paths, and runtime identities before
  proving.
- The two-token local gate deploys two unmodified standard owners with distinct
  derived token IDs, one shared vault key, and one universal bridge VK. It
  registers and activates both records, finalizes both deposits, submits both
  withdrawals, and claims the corresponding Ethereum tokens without generating
  an SP1 proof.

## Settlement cycle optimization benchmark

A separate July 29 source benchmark on the checked-in default mainnet fixture
reduced a full settlement guest execution from 52,159,229,071 to 5,072,572,223
cycles by replacing the zkVM's per-scalar accumulator fallback with an
explicitly serial windowed MSM. The valid proof and verifier hash were identical
across both runs, and a mutated recursive challenge was rejected inside the
guest. The retained bridge rows above are deliberately unchanged until that
distinct two-commit OCaml checkpoint is rerun.

## Needed for the live PoC

1. Build and record immutable machine-local images from the final committed
   source and retained verifier index.
2. Provide a funded Sepolia RPC/admin/gateway identity and a funded Succinct
   requester, deploy the registry module plus settlement and bridge proxies,
   and pass preflight.
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
- Dynamic multi-asset circuit routing within one sequencer and production
  inventory/governance automation.
- Bridge proof fees.
- A canonical Mina verification-key hash; the PoC uses SHA-256 of the exact
  verifier-index JSON bytes.
- Production governance, timelocks, and permissionless proof submission.

The old arbitrary-timeout/ERC20 deposit path and separate withdrawal guest are
retained for compatibility tests but are disabled by default. They are not the
canonical bridge paths described in these docs.
