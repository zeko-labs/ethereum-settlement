# Local E2E

There are four useful local checkpoints. None requires an SP1 proof.

::: warning Resource use
The real settlement guest performs full Pickles verification and takes tens of
minutes of CPU time per commit. Do not replace these commands with `--prove` or
network proving unless that paid/heavy action is explicitly intended.
:::

## 1. Solidity protocol checkpoint

This is the fastest test of contract state transitions:

```sh
cd contracts
forge test --match-path test/NativeBridgePocE2E.t.sol -vv
```

It deploys the implementations behind proxies, locks ETH, accepts an exact
outer Witness action, accepts a later V2 settlement root, rejects an early
claim, advances the virtual slot, and releases ETH with a depth-16 proof. The
SP1 verifier is mocked at the contract boundary.

## 2. Execute one real settlement

Use the low-memory executor against an o1 fixture:

```sh
cargo run --release --bin zkapp -- --execute
```

For a real OCaml fixture, build the guest against its exact verifier index:

```sh
SETTLEMENT_VK_JSON="$PWD/fixtures/zeko-local-e2e/vk.serde.json" \
  cargo run --release --bin vkey
```

The gateway equivalent is `API_EXECUTE_ONLY=true`: it persists the job,
forces the settlement guest through SP1, validates the public values against
live contract state, and stops at `executed` without changing Ethereum. Normal
operational and local-mock settlement jobs use the faster pinned native path.

## 3. Live sequencer and browser bridge checkpoint

Run the browser-facing SDK against a live OCaml sequencer and the production
Actions services:

```sh
tools/run-live-sequencer-bridge-e2e.sh
```

This starts the Zeko testing ledger, a three-node multisig DA quorum, their
signers, one real OCaml prover, and an in-process sequencer GraphQL server. It
also starts the Actions indexer/API pair and a narrow archive compatibility
proxy. The high-level `@zeko-labs/eth-bridge-sdk` then:

1. obtains the deposit witness and commit through the Actions API
2. prepares and signs deposit finalization in the browser-compatible SDK
3. submits the finalization to the real sequencer bridge mutation
4. constructs, signs, and submits an Ethereum-routed native withdrawal request
5. lets the OCaml sequencer commit the resulting inner action
6. validates the two proof-bound settlement exports

This checkpoint exercises the previously mocked browser-to-sequencer boundary.
It does not deploy Ethereum contracts, submit an L1 deposit, finalize an L1
withdrawal, execute SP1, or request an SP1 proof. Those Ethereum-owned steps are
covered by the full round trip below.

The command is CPU- and memory-heavy because it creates real OCaml proofs. For
a run that must survive an SSH disconnect, launch it in `tmux` and retain its
output with `tee`.

## 4. Full native bridge round trip

With a prepared bridge identity and the generated two-commit fixture under
`build/poc/bridge-fixtures`, run:

```sh
tools/run-local-bridge-roundtrip.sh
```

The runner creates isolated Anvil and PostgreSQL instances, starts the gateway,
the outer Actions indexer, and Actions API, and drives user-owned Ethereum
operations through `@zeko-labs/eth-bridge-sdk`. It then:

1. builds the gateway/guest against the fixture verifier index
2. deploys deterministic settlement and bridge proxies with
   `LocalSP1Verifier`
3. discovers chain/contract configuration through the public gateway API and
   locks 10 ETH through the browser SDK
4. indexes the finalized deposit, automatically queues it, and executes the bridge guest
5. submits the validated bridge receipt with empty proof bytes
6. natively verifies and submits the real deposit-synchronizing OCaml settlement
7. natively verifies and submits the real withdrawal-bearing OCaml settlement
8. verifies the deposit Witness is consumable through Actions API
9. obtains the public Merkle path from the gateway
10. advances through the configured withdrawal delay and claims 5 ETH through
    the browser SDK
11. checks contract state, liability, action synchronization, and recipient
    cursor

Mock proof acceptance is hard-limited to the repository verifier on chain ID
31337. Anvil's `finalized` tag does not advance, so the runner explicitly uses
`ETHEREUM_FINALITY_MODE=confirmations` with depth one. Testnet preflight rejects
that fallback. The runner controls Anvil timestamps so the narrow real OCaml
slot windows do not expire while the CPU-heavy execution is running.

To replay the retained machine identity instead of the disposable local
identity, point the runner at `build/poc/testnet-bridge-fixtures` and provide
the matching retained admin address/private key. The runner funds that account
inside Anvil and verifies that its deterministic bridge address is the one
bound into the OCaml circuit. No real funds or external chain are used.

The retained full-zkVM audit checkpoint consumed 3,433,016 cycles for the
bridge and roughly 52.19 billion cycles for each settlement. The normal local
mock path now verifies settlements natively and reports no settlement cycle
count. See [current status](/status) for the recorded audit values.

## Regenerate the OCaml fixture

The fixture must use the same deterministic bridge address and circuit config
as the deployment environment:

```sh
POC_ENV_FILE=deploy/testnet/secrets/fixture-keys.env \
  tools/export-bridge-ocaml-fixtures.sh build/poc/testnet-bridge-fixtures
```

This launches the real sequencer/prover test scenario through Nix, uses three
DA nodes at quorum two, exports exactly two chained settlements, and validates
the bridge address, common VK, synchronized deposit checkpoint, inner-action
length, and withdrawal preimage. It does not request an SP1 proof.

To reuse and revalidate an existing export:

```sh
POC_REUSE_OCAML_EXPORT=true \
  POC_ENV_FILE=deploy/testnet/secrets/fixture-keys.env \
  tools/export-bridge-ocaml-fixtures.sh build/poc/testnet-bridge-fixtures
```

Generated `build/` artifacts are local run products. Promote a reviewed copy to
release storage before treating it as testnet identity.

## Success criteria

- every guest completes and its public values decode under the expected schema
- both settlement source states equal the previous accepted destination state
- the deposit is not reported synchronized before the first settlement
- the V2 inner root matches the gateway-reconstructed tree
- the claim is rejected before delay and succeeds afterward
- the SDK can recover deposit status and claim data using only public APIs
- the browser SDK can finalize a deposit and route a native withdrawal through
  the live sequencer GraphQL API
- the production Actions indexer/API pair accepts the gateway archive shape
- bridge native liability falls by the claimed value
- no Succinct request ID exists in the job records
