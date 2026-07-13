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

1. Reserve the final bridge proxy address, preferably with deterministic
   deployment. Configure the OCaml circuit's `ethereum_holder_account_l1` as
   the compressed key `(x = uint160(proxy), is_odd = false)`. This address is
   proof-bound; changing it requires rebuilding the OCaml bridge VK and SP1
   programs.
2. Build the OCaml outer rules and export their wrap verifier index. Build the
   settlement ELF with `SETTLEMENT_VK_JSON` pointing to that exact JSON. Do not
   deploy an ELF built with the copied o1 example VK.
3. Obtain the SP1 program vkey with the host tooling and deploy an SP1 verifier
   supported by SP1 6.1.
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
   `ZEKO_ETHEREUM_GATEWAY_TOKEN` to the API key. The modified committer sends
   the proof export with its normal `send_zkapp` call.
9. Start with `API_EXECUTE_ONLY=true`. Produce one real OCaml commit and verify
   that the job reaches `executed` and its public values match the initialized
   contract.
10. Fund the Succinct requester account, switch execute-only off, and submit
    one settlement. Confirm the contract state, gateway account view, pending
    pool, actions query, job cost fields and confirmation count.

## Required configuration

- Ethereum: `RPC_URL`, contract addresses, per-role private keys,
  `ETHEREUM_CONFIRMATIONS`, `ETHEREUM_INDEXER_START_BLOCK`.
- SP1: build-time `SETTLEMENT_VK_JSON`, runtime `NETWORK_PRIVATE_KEY`,
  `PROOF_SYSTEM=groth16`,
  timeout, minimum auction period, optional gas/price caps.
- Mina façade: genesis timestamp, fork slot, account-creation fee, initial
  state hash and `VIRTUAL_MINA_ACCOUNTS_PATH`.
- OCaml sequencer: gateway GraphQL URL and `ZEKO_ETHEREUM_GATEWAY_TOKEN`.
- Native bridge: final bridge proxy address in the OCaml circuit config,
  `BRIDGE_CONTRACT_ADDRESS`, `BRIDGE_PRIVATE_KEY`, and
  `VIRTUAL_MINA_OUTER_PUBLIC_KEY`.

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

The 2026-07-13 local checkpoint completed with status `executed` using SP1
program vkey
`0x00160d9427406e3a01391a3887aa481b067a5398f3f003ef52ea10b7d040a602`.
It emitted 768 bytes of public values in 52,200,737,822 cycles, while the
gateway process peaked at roughly 242 MiB RSS. The local settlement contract's
batch sequence remained zero, confirming that execute-only mode made no
Ethereum state change.

## Local native-bridge checkpoint

Run the contract/glue E2E without producing an SP1 proof:

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
comes from the Rust guest tests and `ethereum_bridge_vectors.exe`, which asserts
the same native-deposit Poseidon aux values in OCaml. A fully live testnet still
needs freshly generated OCaml deposit/withdraw/commit fixtures built for the
chosen bridge address and real deployed SP1 vkeys.
