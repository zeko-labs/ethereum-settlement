# Local E2E

There are three useful local checkpoints. None requires an SP1 proof.

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
executes SP1, validates the public values against live contract state, and
stops at `executed` without changing Ethereum.

## 3. Full native bridge round trip

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
6. executes and submits the real deposit-synchronizing OCaml settlement
7. executes and submits the real withdrawal-bearing OCaml settlement
8. verifies the deposit Witness is consumable through Actions API
9. obtains the public Merkle path from the gateway
10. advances through the configured withdrawal delay and claims 5 ETH through
    the browser SDK
11. checks contract state, liability, action synchronization, and recipient
    cursor

Mock proof acceptance is hard-limited to the repository verifier on chain ID
31337. The runner controls Anvil timestamps so the narrow real OCaml slot
windows do not expire while the CPU-heavy execution is running.

The latest recorded checkpoint consumed 3,433,016 cycles for the bridge and
roughly 52.19 billion cycles for each settlement. See [current status](/status)
for the exact values.

## Regenerate the OCaml fixture

The fixture must use the same deterministic bridge address and circuit config
as the deployment environment:

```sh
tools/export-bridge-ocaml-fixtures.sh build/poc/bridge-fixtures
```

This launches the real sequencer/prover test scenario through Nix, uses three
DA nodes at quorum two, exports exactly two chained settlements, and validates
the bridge address, common VK, synchronized deposit checkpoint, inner-action
length, and withdrawal preimage. It does not request an SP1 proof.

To reuse and revalidate an existing export:

```sh
POC_REUSE_OCAML_EXPORT=true \
  tools/export-bridge-ocaml-fixtures.sh build/poc/bridge-fixtures
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
- the production Actions indexer/API pair accepts the gateway archive shape
- bridge native liability falls by the claimed value
- no Succinct request ID exists in the job records
