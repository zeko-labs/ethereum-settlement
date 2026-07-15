# Multisig-DA Testnet PoC

This is the deployment runbook and acceptance boundary for the no-blob PoC.
The OCaml Zeko circuits remain authoritative. Multisig DA is retained for this
testnet milestone only; EIP-4844 remains the production direction.

## Data flow

```text
Zeko sequencer
  -> Mina-compatible /graphql sendZkapp
  -> gateway PostgreSQL queue
  -> local SP1 execute/preflight
  -> Succinct Network Groth16 proof
  -> ZekoSettlement.verifyAndUpdateRoot
  -> confirmation/reorg indexer
  -> Mina-compatible account, pool, action and bestChain reads
```

The sequencer exports its actual Pickles proof, verifier index, full Pickles
statement skeleton, two-field zkApp statement, account-update body hash
preimage, action fields, and source outer state. SP1 verifies Pickles and
derives the settlement receipt. The gateway only supplies Ethereum-domain
context, which the contract checks against its own state.

The native bridge adds two proof-bound paths:

```text
ETH deposit -> BridgeDeposit log -> finalized gateway batch -> bridge SP1
  -> exact outer Witness actions -> sequencer Mina actions view
  -> later synchronized settlement checkpoint

OCaml inner Witness actions -> settlement SP1 Keccak tree
  -> accepted settlement V2 root -> gateway Merkle path
  -> delayed permissionless ETH claim
```

## Build and deploy order

1. Generate the retained admin and predict the final bridge proxy with
   `tools/init-machine-testnet-identity.sh`. The CREATE2 factory later deploys
   and initializes each proxy atomically. Configure the OCaml circuit's
   `ethereum_holder_account_l1` as
   the compressed key `(x = uint160(proxy), is_odd = false)`. This address is
   proof-bound; changing it requires rebuilding the OCaml bridge VK and SP1
   programs.
2. Export the real two-commit OCaml bridge scenario with the retained circuit,
   sequencer, recipient, and three DA identities. Both OCaml chains and the
   external signer use Mina network ID `testnet` for Auro compatibility. Build
   the OCaml outer rules and export their wrap verifier index. Build the
   settlement ELF with `SETTLEMENT_VK_JSON` pointing to that exact JSON. Do not
   deploy an ELF built with the copied o1 example VK.
3. Run `tools/prepare-poc.sh` against Sepolia to obtain the SP1 program vkeys
   and bind the manifest to the official SP1 6.1 Groth16 verifier. Preparation
   does not prove or deploy.
4. Deploy `ZekoSettlement` and `EthereumZekoBridge` behind `ERC1967Proxy`.
   Initialize the full eight outer-state fields, action state/length, genesis
   timestamp, slot duration, fork slot, program vkey and SHA-256 PoC VK
   identifier.
5. Grant `PROVER_ROLE` only to the gateway settlement signer. Keep admin and
   upgrader keys separate.
6. Create `virtual-mina-accounts.json` with complete GraphQL account objects
   for the outer account and sequencer fee payer. Values must match the
   contract genesis state.
7. Configure and start PostgreSQL plus the gateway. Set the Ethereum indexer
   start block to the settlement deployment block. When using Compose, add a
   read-only bind mount for the accounts file. Set
   `VIRTUAL_MINA_ACCOUNTS_PATH` to its container path and
   `VIRTUAL_MINA_OUTER_PUBLIC_KEY` to the rollup outer account.
8. Point the sequencer L1 GraphQL URI at `<gateway>/graphql` and set
   `ZEKO_ETHEREUM_GATEWAY_TOKEN` to the API key and
   `ZEKO_SIGNATURE_KIND=testnet`. The modified committer sends the proof export
   with its normal `send_zkapp` call.
9. Start with `API_EXECUTE_ONLY=true`. Produce one real OCaml commit and verify
   that the job reaches `executed` and its public values match the initialized
   contract.
10. Fund the Succinct requester account and switch execute-only off with
    `API_REQUIRE_PROOF_APPROVAL=true`. Let the job complete local execution,
    inspect `GET /v1/proofs/:id/quote`, then approve the exact input digest with
    explicit per-job PGU and price caps. Confirm the contract state, gateway
    account view, pending pool, actions query, cost fields and confirmation
    count.

## Required configuration

- Ethereum: `RPC_URL`, contract addresses, per-role private keys,
  `ETHEREUM_CONFIRMATIONS`, `ETHEREUM_INDEXER_START_BLOCK`.
- SP1: build-time `SETTLEMENT_VK_JSON`, runtime `NETWORK_PRIVATE_KEY`,
  `PROOF_SYSTEM=groth16`,
  timeout, minimum auction period, optional gas/price caps.
- Mina façade: genesis timestamp, fork slot, account-creation fee, initial
  state hash and `VIRTUAL_MINA_ACCOUNTS_PATH`.
- OCaml sequencer: gateway GraphQL URL and `ZEKO_ETHEREUM_GATEWAY_TOKEN`.
- Browser signing: `MINA_SIGNING_NETWORK_ID=testnet` everywhere; Auro's custom
  network display name is not a signing salt.
- Native bridge: final bridge proxy address in the OCaml circuit config,
  `BRIDGE_CONTRACT_ADDRESS`, `BRIDGE_PRIVATE_KEY`, and
  `VIRTUAL_MINA_OUTER_PUBLIC_KEY`.

The generated `build/poc/manifest.json` is the canonical public identity for a
run: chain ID, deterministic contract addresses, all three SP1 vkeys, the PoC
Pickles VK hash, and the exact 160-bit holder value compiled by OCaml. Review it
before deployment and retain a copy with the testnet run artifacts.

`tools/prepare-poc.sh` also builds the gateway with the selected fixture VK.
Gateway startup recomputes all embedded program vkeys and refuses to run if any
differ from the live contracts. Preparation derives `FORK_SLOT` from the
fixture's proof-bound lower slot so the deployment and gateway cannot silently
start at slot zero for a nonzero commit range.

The pinned persistent profile and secret/bootstrap workflow live under
`deploy/testnet/`. It runs one real sequencer/prover, three retained DA nodes at
quorum two, the gateway, both databases, RabbitMQ, isolated signers, bounded DA
bootstrap, and a prover-readiness barrier. Run `tools/testnet-preflight.sh`
before starting it; tags, mismatched vkeys/roles/identities, bypass modes, and
non-Sepolia RPCs are rejected.

The standalone React application lives in `bridge-ui/`. It uses public gateway,
sequencer/archive, and Actions endpoints for deposit, deposit finalization,
withdrawal request, and claim flows. It must never receive the proof operator
API key.

## Acceptance tests

- A genuine OCaml commit executes in SP1 and a mutated app statement, deferred
  value, bulletproof challenge, accumulator point, feature flag or previous
  evaluation fails.
- A changed body field or action fails before public values are emitted.
- Wrong chain, contract, VK, batch, source state/action length, or slot range
  reverts in Solidity.
- Restarting the gateway resumes an existing proof request by ID.
- A submitted transaction becomes confirmed only at the configured depth.
- A reorg restores virtual accounts/actions, puts the command back in the
  pending pool and resubmits the already-paid proof.
- Two queued settlements receive Ethereum context serially; the second cannot
  reserve or purchase a proof for the same next batch.

## Explicit PoC limits

- The VK identifier is SHA-256 of verifier-index JSON, not the canonical Mina
  VK hash.
- Multisig DA is not replaced by blobs in this milestone.
- Native ETH deposits and delayed native withdrawals are implemented. Deposit
  cancellation/timeouts, proof fees, and ERC20 bridging are intentionally out
  of scope; their legacy entry points remain disabled by default.
- `fixtures/zeko-local-e2e` is a genuine OCaml outer-commit fixture generated by
  `test_all_real`. It is suitable for the local execute-only checkpoint, but it
  is not a stable testnet genesis or production fixture.

## Local execute-only checkpoint

The real fixture uses optional Kimchi gates and lookup columns, so it must be
run with the feature-aware Pickles verifier. Rebuild the settlement ELF and
record its vkey whenever verifier code changes:

```sh
SETTLEMENT_VK_JSON="$PWD/fixtures/zeko-local-e2e/vk.serde.json" \
  cargo run --release --bin vkey
```

Start PostgreSQL and Anvil, then deploy the local proxy with
`contracts/script/DeployLocalSettlement.s.sol`. Set the eight
`INITIAL_OUTER_STATE_<n>` variables from `proof.binding.stateBefore.fields`,
`INITIAL_OUTER_ACTION_STATE` from account-update body field 36,
`SETTLEMENT_VK_HASH` to the SHA-256 digest of `vk.serde.json`, and
`SETTLEMENT_PROGRAM_VKEY` to the command output. The script prints the proxy
address used as `SETTLEMENT_CONTRACT_ADDRESS`.

Run the gateway with `API_EXECUTE_ONLY=true`, then submit the fixture as the
`settlement` GraphQL variable:

```sh
jq -n --slurpfile settlement fixtures/zeko-local-e2e/settlement.json \
  '{query:"mutation { sendZkapp { zkapp { id failureReason } } }",
    variables:{gatewayToken:"local-e2e-key",settlement:$settlement[0]}}' \
  | curl -H 'content-type: application/json' --data-binary @- \
      http://127.0.0.1:8080/graphql

curl -H 'x-api-key: local-e2e-key' http://127.0.0.1:8080/v1/proofs
```

This path verifies the real Pickles proof inside SP1 and validates the emitted
public values against live contract state. It does not generate an SP1 proof
or send an Ethereum settlement transaction.

To advance the local contracts without proving, deploy with
`LOCAL_MOCK_VERIFIER=true` and replace `API_EXECUTE_ONLY=true` with
`API_LOCAL_MOCK_SUBMIT=true`. The gateway still executes and validates the
guest first, then submits its public values with an empty proof to the marked
local verifier. This mode is hard-limited to chain ID 31337 and is the local
path for testing consecutive commits and bridge synchronization.
`DeployPoc` holds the local mock deployment at `FORK_SLOT` for one day by
default so a long Pickles preflight cannot age past the proof's slot range;
set `GENESIS_TIMESTAMP` explicitly to override that test-only clock. The E2E
runner mines only after Ethereum submission, never while SP1 is executing.

With Anvil, the deterministic contracts, and the `zeko-poc-postgres` container
running, the canonical native-deposit portion is automated by:

```sh
tools/run-local-deposit-e2e.sh
```

It creates an isolated database, locks 1 ETH, waits for the indexed finalized
log, executes the bridge guest, submits only to `LocalSP1Verifier`, waits for
the transaction to confirm, and checks the proof-bound outer action. It never
requests or generates an SP1 proof.

Generate genuine consecutive OCaml commits with the real sequencer prover and
three-node multisig DA stack by running:

```sh
tools/export-sequential-ocaml-fixtures.sh
```

The sequencer writes each fully signed gateway submission before sending it to
the testing L1. The wrapper requires at least three exports, checks that every
next `stateBefore` equals the prior proof-bound update, and materializes each
sequence as a fixture directory under `build/poc/sequential-fixtures`. It also
requires one common VK and the deterministic bridge address, and stops the OCaml
test executable after its bounded three-commit scenario. These can be replayed
in order through `tools/run-local-settlement-e2e.sh` after the first fixture is
used to prepare and initialize a fresh deployment. The runner configures the
outer account and a distinct fee-payer account when the fixture uses both.

The 2026-07-13 local checkpoint completed with status `executed` using SP1
program vkey
`0x00160d9427406e3a01391a3887aa481b067a5398f3f003ef52ea10b7d040a602`.
It emitted 768 bytes of public values in 52,200,737,822 cycles, while the
gateway process peaked at roughly 242 MiB RSS. The local settlement contract's
batch sequence remained zero, confirming that execute-only mode made no
Ethereum state change.

The 2026-07-14 generated-sequence checkpoint used multisig DA quorum 2 of 3 and
verification-key SHA-256
`0x2a5c1cb5b3e2d16b213a638d63e77c03c7d82bca8f61acc9a0335c3a7f16ddb8`.
Its settlement program vkey was
`0x00c060a53019c46e433aa8da7add97853e0830e30d0e1eba4eefcbba535e418d`.
Sequence 0 executed in 52,188,766,765 cycles and emitted an 828-byte V2 receipt.
The gateway submitted that receipt with an empty proof only to the marked Anvil
verifier; transaction
`0x698ab5d004a7a04643f044eeb4950adf4732ae75d56a6df3336706613f3bc88a`
succeeded with 295,773 gas. Batch sequence advanced from 0 to 1, action-state
length advanced to 1, and all eight stored outer-state fields matched the
proof-bound account update. No SP1 proof was generated.

## Local native-bridge checkpoint

Generate the real two-commit bridge fixture and run the complete local
roundtrip without producing an SP1 proof:

```sh
tools/export-bridge-ocaml-fixtures.sh build/poc/bridge-fixtures
tools/run-local-bridge-roundtrip.sh
```

The runner creates fresh Anvil and PostgreSQL instances, deploys the real
settlement and bridge implementations behind deterministic proxies, locks 10
ETH, executes the bridge guest, executes both OCaml-produced settlements,
checks the synchronized deposit, moves Anvil to each proof's lower slot and
then through the withdrawal delay, obtains the public depth-16 Merkle path from
the gateway, and claims the 5 ETH withdrawal. Empty proof bytes are accepted
only by the marked chain-31337 `LocalSP1Verifier`; all three guests execute and
validate their public values first.

The 2026-07-14 native round-trip checkpoint used the real two-commit OCaml
export at DA quorum 2 of 3. The bridge guest executed in 3,433,016 cycles and
used 218,288 Ethereum gas. The deposit-synchronizing settlement executed in
52,186,638,600 cycles and used 275,837 gas; the withdrawal-bearing settlement
executed in 52,189,369,576 cycles and used 318,590 gas. Both settlements
confirmed, the gateway indexed the withdrawal preimage/root, and the depth-16
claim transaction released the configured 5 ETH. The runner fixes Anvil's
block-timestamp interval while SP1 is executing so the real 20-slot OCaml
window neither stalls below its lower bound nor expires through wall-clock
drift. No SP1 proof or network request was generated.

The 2026-07-15 retained-identity checkpoint regenerated that scenario under
the exact Mina `testnet` signing and DA receipt domain, with deterministic
bridge `0xf77f7f4Fb6287dd5F36831E80a5AFa59b590E023` and verifier-index SHA-256
`a9bca935bad09638d2e335a7cfc4ecc110389d15c033db6b0413593ca9193c74`.
The browser SDK deposit and production Actions services were part of the same
run. The bridge guest executed in 3,435,291 cycles and used 218,336 gas. The
deposit-synchronizing settlement executed in 52,191,513,620 cycles and used
275,825 gas; the withdrawal-bearing settlement executed in 52,187,890,758
cycles and used 318,602 gas. The depth-16 claim released 5 ETH, all escrow
liability checks passed, and the recorded result has `sp1ProofsGenerated: 0`.

The faster Solidity component checkpoint remains:

```sh
cd contracts
forge test --match-path test/NativeBridgePocE2E.t.sol -vv
```

The test deploys the real settlement and bridge implementations behind proxies,
locks 1 ETH, accepts a bridge V2 receipt containing the exact outer Witness
action, accepts a later settlement V2 receipt that synchronizes that outer
checkpoint and commits a Keccak tree over exact inner actions, rejects an early
claim, advances the virtual Mina slot, and releases the ETH with a depth-16
Merkle proof. It checks the bridge liability and per-recipient cursor after the
claim.

The verifier is mocked only at the SP1 contract boundary. Proof-side coverage
comes from the executed guests, Rust negative tests, and
`ethereum_bridge_vectors.exe`, which asserts the same native-deposit Poseidon
aux values in OCaml. A live testnet must regenerate the fixture with retained
identities, a 2400-slot validity period, the final bridge address, and the exact
SP1 vkeys used for deployment.

## Native bridge user flow

The no-cancellation native PoC has three network proof boundaries but neither
user-side proving nor an automatic relayer:

1. A user calls `depositNative` and waits for Ethereum finality. The public
   `GET /v1/bridge/deposits/:nonce` endpoint reports `requestBridgeProof`.
2. An operator manually calls `POST /v1/bridge/deposits/prove`. The bridge SP1
   proof appends exact outer Witness actions. The following real OCaml commit
   synchronizes them; the deposit endpoint then reports
   `finalizeDepositOnZeko`.
3. The user asks the Zeko bridge prover API for `finalizeDeposit`, signs the
   helper-account update whose commitment includes the final action state, and
   submits it to the sequencer. The gateway does not forge this signature.
4. The user similarly requests, signs, and submits the L2 withdrawal
   transaction. A later genuine OCaml settlement binds its inner Witness action
   into the V2 Keccak root.
5. `GET /v1/bridge/withdrawals?recipient=0x...` returns the fixed-depth Merkle
   path plus live delay/cursor status. Once it reports `claimable`, the user
   calls `claimNativeWithdrawal`; no Mina proof or SP1 wrap is required.

Run every genuine fixture through `API_EXECUTE_ONLY=true` first. Only after all
three jobs execute and their public values match the live Anvil state should a
Succinct Network quote be requested. Network proof creation remains a separate
paid approval step.

Current auction parameters can be read without creating a proof request:

```sh
cargo run --release --bin network_quote -- --proof-system groth16
# Once a preflight or network simulation has supplied PGUs:
cargo run --release --bin network_quote -- --proof-system groth16 --pgu <pgu>
```

The quote reports the base fee, maximum price per PGU, and optional maximum
charge in PROVE. It is read-only; the paid boundary remains the gateway's
network proof request.
