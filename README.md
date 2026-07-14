# Zeko Ethereum L2

This repository contains SP1 programs and Ethereum contracts used to settle Zeko state transitions on Ethereum.

The project has three verification paths:

- **Settlement circuit**: verifies a Zeko/o1 proof for a zkApp command and commits the rollup state transition that Ethereum should accept.
- **Bridge circuit**: verifies the Ethereum-to-Zeko bridge transition by replaying deposits, updating the Ethereum deposit accumulator, and computing the Zeko action state expected by the Zeko bridge account.
- **Withdraw circuit**: verifies the Zeko-to-Ethereum withdraw transition by computing a fixed-depth withdrawal Merkle root, deriving the Ethereum withdrawal state from that root, and computing the Zeko action state for the withdraw batch.

The goal is to let Ethereum verify succinct SP1 proofs instead of directly verifying the full Zeko/o1 proof system or re-executing bridge action-state logic on-chain.

## Documentation

The VitePress documentation lives in [`docs/content`](docs/content).

## Launching A Zeko Rollup

The project includes a dedicated Docker Compose stack for running the Zeko
rollup services described in the Zeko operator guide.

[https://docs.zeko.io/operators/guides/launch-rollup.html](https://docs.zeko.io/operators/guides/launch-rollup.html)

```sh
docker compose -f docker-compose.yaml up -d
docker compose -f docker-compose.yaml exec -it init-config bash
docker compose -f docker-compose.yaml exec -it init-deploy bash
docker compose -f docker-compose.yaml logs -f
```

### Cloudflare Pages

Import this GitHub repository from **Workers & Pages > Create application >
Pages > Import an existing Git repository**, then use:

| Setting | Value |
| --- | --- |
| Production branch | `main` (or the branch used for production) |
| Root directory | `docs` |
| Build command | `pnpm build` |
| Build output directory | `.vitepress/dist` |

Cloudflare Pages installs the dependencies from `docs/package.json`. Each push
to the production branch publishes a new production deployment; pushes to
other branches create preview deployments.

## Repository Layout

| Path | Purpose |
| --- | --- |
| `program/settlement` | SP1 guest program that verifies a Pickles proof using the o1 `pickles-verifier` path and emits settlement public values. |
| `program/bridge` | SP1 guest program that verifies bridge deposits and computes Ethereum/Zeko deposit accumulator transitions. |
| `program/withdraw` | SP1 guest program that verifies bridge withdrawals and computes Ethereum/Zeko withdrawal-state transitions. |
| `lib` | Shared Rust input/output types used by guests and host scripts. |
| `script` | Host-side proof generation and execution binaries. |
| `crates/pickles-verifier` | o1 reference Pickles verifier adapted for this workspace. |
| `contracts/src/ZekoSettlement.sol` | Ethereum verifier wrapper for settlement proofs. |
| `contracts/src/EthereumZekoBridge.sol` | Ethereum-side bridge contract that records deposits and accepts withdraw states. |
| `tools/zeko-action-state` | o1js fixture that reproduces Zeko action-state updates for bridge deposits. |
| `proofs/bridge-input.json` | Example bridge input fixture. |
| `proofs/bridge-input-200.json` | Bridge input fixture with 200 deposits. |
| `proofs/withdraw-input.json` | Withdraw input fixture with 3 withdrawals. |
| `proofs/withdraw-input-200.json` | Withdraw input fixture with 200 withdrawals. |

## Contracts, Proxies And Roles

`ZekoSettlement` and `EthereumZekoBridge` are UUPS implementations intended to be deployed behind OpenZeppelin `ERC1967Proxy` proxies. Deploy a fresh implementation, then deploy an `ERC1967Proxy` with the encoded `initialize(...)` call as constructor data.

Both contracts use OpenZeppelin `AccessControl` with the same role layout:

- `DEFAULT_ADMIN_ROLE`: grants and revokes roles.
- `ADMIN_ROLE`: contract administration such as token configuration, pausing, emergency withdrawals, and settlement parameter updates.
- `PROVER_ROLE`: submits SP1 proof transitions to the contracts.
- `UPGRADER_ROLE`: authorizes UUPS implementation upgrades.

The initializer grants all four roles to the initial admin. Proof submission is intentionally separated from admin operations so relayers can be permissioned without giving them upgrade or configuration rights.

## Settlement Circuit

The settlement program in `program/settlement` verifies a Mina/Pickles proof
inside SP1 using the o1 `o1js-to-zkvm` verifier design.

At a high level it:

1. Builds a verifier blob at compile time from
   `proofs/mainnet-blockchain-snark/vk.serde.json` or `SETTLEMENT_VK_JSON`.
2. Host code reads an o1 fixture directory containing:
   - `vk.serde.json`
   - `proof.serde.json`
   - `public_input_skeleton.json`
   - `app_statement.json`
3. Host code converts those OCaml/o1 files into a `VerifiableProof`.
4. The SP1 guest verifies:
   - Pickles accumulator / challenge polynomial commitment
   - deferred value reconstruction
   - wrap public input reconstruction
   - outer Kimchi proof
5. For a real Zeko export, the guest recomputes the account-update body digest,
   checks it against the verified two-field zkApp statement, recomputes the
   action hash, and decodes the fixed eight-field outer `Commit` action.
6. The guest derives and emits the versioned 768-byte settlement receipt. A
   fixture-only compatibility mode still emits the old 577-byte output so the
   copied o1 fixtures can be executed without pretending they are Zeko commits.

The V1 receipt contains the complete eight-field outer state, current and
synchronized outer action states and lengths, inner action state and length,
slot range, Ethereum domain, batch sequence, and Mina transaction tracking
hash. The Ethereum domain values are supplied by the gateway and checked by
Solidity; the state transition is derived from data committed by Pickles.

The PoC VK identifier remains SHA-256 over the exact verifier-index JSON baked
into the guest. A production deployment must switch this to the canonical
OCaml/Mina verification-key hash.

On Ethereum, `ZekoSettlement.sol` verifies the SP1 proof and checks that the
public output matches the verifier contract's tracked state:

- chain ID, settlement address, batch sequence and VK hash must match L1;
- all eight source state fields and the outer action state/length must match;
- the action length must increment exactly once and the synchronized length may
  not exceed the committed outer length;
- the virtual Mina slot must be inside the proved commit range.

On acceptance it stores the complete next outer state and records the accepted
inner action state for bridge consumers.

## Bridge Circuit

The bridge program in `program/bridge` proves that a batch of Ethereum deposits maps to the expected Zeko action-state transition. It is deposit-only; withdrawals are handled by `program/withdraw`.

For each deposit, the program:

1. Validates and unpacks the packed `ZekoAddress` into `(x, isOdd)`.
2. Converts the deposit amount into the Zeko amount field.
3. Computes the Ethereum deposit leaf:

```text
keccak256(
  ZEKO_BRIDGE_DEPOSIT_LEAF_V1,
  chain_id,
  bridge_address,
  token,
  zeko_recipient,
  zeko_amount,
  timeout,
  nonce
)
```

4. Updates the Ethereum deposit accumulator:

```text
keccak256(
  ZEKO_BRIDGE_DEPOSIT_STATE_V1,
  previous_deposit_state,
  deposit_leaf
)
```

5. Computes the native Ethereum deposit aux value:

```text
Poseidon.hashWithPrefix("Ethereum deposit V1", [
  emptyCallForest,
  bridgeAddress.x,
  false,
  zekoAmount,
  recipient.x,
  recipient.isOdd,
  UInt32.max
])
```

6. Emits the exact five-field outer Witness action
   `[1, aux, 0, 0, UInt32.max]`, then adds it to the Zeko action-state sequence
   using Mina Poseidon semantics.

The bridge public output includes:

- Ethereum deposit state before/after
- Ethereum nonce before/after
- Zeko action state before/after
- Zeko action-state length before/after
- every exact five-field action and its intermediate action-state checkpoint

The native path accepts ETH only, requires 1 gwei granularity, fixes the timeout
to `UInt32.max`, and rejects an empty batch. Arbitrary-timeout and ERC20 deposit
entry points are disabled by default because the current OCaml PoC cannot safely
consume or cancel them.

## Native Bridge PoC

Settlement public values V2 bind the exact ordered inner actions to the
proof-verified inner action-state transition. SP1 emits a depth-16 Keccak root,
the global start index, count, bridge address, and the normal settlement
receipt. Solidity records that root only while accepting the corresponding
settlement transition. A native withdrawal claim supplies an ordinary Merkle
proof, amount, recipient, and action-fields hash; it does not require the user
to generate a SNARK.

The OCaml Ethereum deposit rule recognizes one additional synthetic holder key:
`x = uint160(EthereumZekoBridge)` and `is_odd = false`. Its circuit configuration
must contain the exact final bridge proxy address before the OCaml bridge VK and
SP1 ELF are built. Use a predetermined deployment address (for example CREATE2)
or deploy the proxy first, then build the circuit artifacts against it.

Run the contract/glue checkpoint locally:

```sh
cd contracts
forge test --match-path test/NativeBridgePocE2E.t.sol -vv
```

This locks native ETH, imports the deposit as an exact outer Witness action,
accepts a later settlement that synchronizes that checkpoint and binds an inner
withdrawal tree, enforces the configured delay, and releases ETH with a Merkle
claim. The test uses a mock SP1 verifier at the contract boundary; Rust guest
tests and OCaml cross-language vectors cover the two proof-side encodings
without generating an SP1 proof.

## Withdraw Circuit

The withdraw program in `program/withdraw` proves that a batch of Zeko withdrawals maps to a fixed-depth withdrawal Merkle root, the corresponding Ethereum withdrawal state, and the expected Zeko action-state transition.

For each withdraw, the program:

1. Computes the Ethereum withdraw leaf:

```text
keccak256(
  ZEKO_BRIDGE_WITHDRAW_LEAF_V1,
  chain_id,
  bridge_address,
  token,
  recipient,
  amount
)
```

2. Builds the fixed-depth withdrawal Merkle root, then updates the Ethereum
   withdrawal state once for the complete batch:

```text
keccak256(
  ZEKO_BRIDGE_WITHDRAW_STATE_V1,
  previous_withdraw_state,
  withdrawal_root,
  withdraw_count
)
```

3. Computes the Zeko withdraw action:

```text
Poseidon.hashWithPrefix("Withdrawal_params - qFB3jXP*)", [
  Field(0),
  amount,
  recipient
])
```

4. Adds that action to the Zeko action-state sequence.

The withdraw public output includes:

- Zeko action state before/after
- Ethereum withdraw state before/after
- withdrawal Merkle root
- withdraw count

The `tools/zeko-action-state` fixture deploys a local o1js contract and dispatches the same deposit actions, so the SP1 bridge output can be compared against a real action-state update.

## Testing

Run the native o1 Pickles verifier tests over the copied fixture matrix:

```sh
cargo test -p pickles-verifier
```

Run the bridge unit tests (includes real on-chain data replay against testnet state):

```sh
cargo test --manifest-path program/bridge/Cargo.toml
```

Run the withdraw unit tests (same real L2 inner-action data):

```sh
cargo test --manifest-path program/withdraw/Cargo.toml
```

Run the settlement receipt binding tests:

```sh
cargo test -p settlement-program
```

Run specific bridge/withdraw tests:

```sh
cargo test --manifest-path program/bridge/Cargo.toml real_l1_outer_witness
cargo test --manifest-path program/bridge/Cargo.toml real_l2_inner_actions
cargo test --manifest-path program/withdraw/Cargo.toml real_l2_inner_actions
```

The real-data bridge tests replay on-chain state transitions from:
- L2 inner actions (withdrawals): `https://testnet.zeko.io/graphql` — contract `B62qjDedeP9617oTUeN8JGhdiqWg4t64NtQkHaoZB9wyvgSjAyupPU1`
- L1 outer witness actions (deposits): `https://testnet.api.actions.zeko.io/graphql` — contract `B62qkekmS9273D1EsFfMSJMMDAmgvh1WyoYE2vs1r7k4GtGBqVYABn2`

## Running Circuits Without Proving

Execute the settlement program without proving:

```sh
cargo run --release --bin zkapp -- --execute
```

Use a different o1 fixture directory:

```sh
cargo run --release --bin zkapp -- --execute --fixture-dir fixtures/nrr
```

Execute the bridge program without proving:

```sh
cargo run --release --bin bridge -- --execute
```

Execute the 200-deposit bridge fixture:

```sh
cargo run --release --bin bridge -- --execute --input proofs/bridge-input-200.json
```

Execute the withdraw program without proving:

```sh
cargo run --release --bin withdraw -- --execute
```

Execute the 200-withdraw fixture:

```sh
cargo run --release --bin withdraw -- --execute --input proofs/withdraw-input-200.json
```

Run the o1js action-state fixture:

```sh
cd tools/zeko-action-state
npm install
npm start
```

Current fixture output for three deposits:

```text
zeko_action_before: 0x3772bc5435b957f81f86f752e93f2e29e886ac24580b3d1ec879c1dad26965f9
zeko_action_after : 0x3d638b908c4241e7b417d1790a79d0fe3277a133a5a87e12a484cd756de795bf
nonce_after       : 3
deposit_count     : 3
```

## Proof API

The asynchronous Rust API accepts settlement, bridge, and withdraw proof jobs,
checks their Ethereum preconditions, requests EVM-compatible proofs from the SP1
Network, simulates contract submission, and broadcasts valid transactions. Its
native deposit endpoint derives proof input from canonical finalized Ethereum
logs, and its public withdrawal endpoint serves settlement-bound Keccak paths.

It can run with Docker Compose using a read-only environment-file mount and a
persistent PostgreSQL volume. See [`api/README.md`](api/README.md) and
[`.env.api.example`](.env.api.example).

For the multisig-DA testnet architecture and deployment order, see
[`TESTNET_POC.md`](TESTNET_POC.md).

## Generating Proofs

Generate an EVM-compatible Groth16 proof:

```sh
cargo run --release --bin evm -- --system groth16
```

Generate a PLONK proof:

```sh
cargo run --release --bin evm -- --system plonk
```

Use a different settlement fixture:

```sh
cargo run --release --bin evm -- --system groth16 --fixture-dir fixtures/nrr
```

Retrieve the settlement program verification key:

```sh
cargo run --release --bin vkey
```

To read the current network fee parameters and calculate a maximum-cost bound
without registering a program or requesting a proof:

```sh
cargo run --release --bin network_quote -- --proof-system groth16
# After SP1 simulation reports a PGU value:
cargo run --release --bin network_quote -- --proof-system groth16 --pgu <pgu>
```

The optional total is an upper bound (`base fee + PGU × current maximum
price`). Raw executor cycles are not a substitute for network PGU; use the
value returned by SP1 simulation or a proof request. Actual auction cost can be
lower. No static PROVE estimate is kept in the repository because the market
price changes. The command only reads auction parameters and never requests a
proof.
