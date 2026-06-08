# Zeko Ethereum L2

This repository contains SP1 programs and Ethereum contracts used to settle Zeko state transitions on Ethereum.

The project has three verification paths:

- **Settlement circuit**: verifies a Zeko/o1 proof for a zkApp command and commits the rollup state transition that Ethereum should accept.
- **Bridge circuit**: verifies the Ethereum-to-Zeko bridge transition by replaying deposits, updating the Ethereum deposit accumulator, and computing the Zeko action state expected by the Zeko bridge account.
- **Withdraw circuit**: verifies the Zeko-to-Ethereum withdraw transition by replaying withdrawals, updating the Ethereum withdraw accumulator, computing a fixed-depth withdrawal Merkle root, and computing the Zeko action state for the withdraw batch.

The goal is to let Ethereum verify succinct SP1 proofs instead of directly verifying the full Zeko/o1 proof system or re-executing bridge action-state logic on-chain.

## Documentation

The VitePress documentation lives in [`docs/content`](docs/content).

### Cloudflare Pages

Import this GitHub repository from **Workers & Pages > Create application >
Pages > Import an existing Git repository**, then use:

| Setting | Value |
| --- | --- |
| Production branch | `bridge` (or the branch used for production) |
| Root directory | `docs` |
| Build command | `pnpm build` |
| Build output directory | `.vitepress/dist` |

Cloudflare Pages installs the dependencies from `docs/package.json`. Each push
to the production branch publishes a new production deployment; pushes to
other branches create preview deployments.

## Repository Layout

| Path | Purpose |
| --- | --- |
| `program/settlement` | SP1 guest program that verifies a Zeko/o1 proof and extracts canonical settlement public values. |
| `program/bridge` | SP1 guest program that verifies bridge deposits and computes Ethereum/Zeko deposit accumulator transitions. |
| `program/withdraw` | SP1 guest program that verifies bridge withdrawals and computes Ethereum/Zeko withdraw accumulator transitions. |
| `lib` | Shared Rust input/output types used by guests and host scripts. |
| `script` | Host-side proof generation and execution binaries. |
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

The settlement program in `program/settlement` verifies a Zeko/o1 proof inside SP1.

At a high level it:

1. Reads the Zeko verification key, o1 proof, zkApp statement, zkApp command, deferred values, and verifier index.
2. Rebuilds the verifier index with the embedded SRS.
3. Checks that the zkApp command is bound to the statement being verified.
4. Runs Kimchi verification for the supplied proof.
5. Extracts public values from the first account update:
   - proof validity flag
   - verification-key hash
   - zkApp state before
   - zkApp state after
   - action state before
6. Commits those public values as SP1 public output.

On Ethereum, `ZekoProofVerifier.sol` verifies the SP1 proof and checks that the public output matches the verifier contract's tracked state:

- `vkHash` must match the expected Zeko verification key hash.
- `actionStateBefore` must match the verifier's stored action state.
- `stateBefore[3]` must match the verifier's current root.
- `stateAfter[3]` becomes the new root.

This contract currently updates the settlement root. It stores action state as a guard input but does not derive a new action state from the settlement proof output.

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

5. Computes the Zeko deposit action:

```text
Poseidon.hashWithPrefix("Deposit_params - qFB3jXP*)", [
  Field(0),
  holderAccountL1,
  zekoAmount,
  recipient.x,
  recipient.isOdd,
  timeout
])
```

6. Adds that action to the Zeko action-state sequence using the same Poseidon update semantics as o1js.

The bridge public output includes:

- Ethereum deposit state before/after
- Ethereum nonce before/after
- Zeko action state before/after
- deposit count

## Withdraw Circuit

The withdraw program in `program/withdraw` proves that a batch of Zeko withdrawals maps to the expected Ethereum withdraw accumulator and Zeko action-state transition.

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

2. Updates the Ethereum withdraw accumulator:

```text
keccak256(
  ZEKO_BRIDGE_WITHDRAW_STATE_V1,
  previous_withdraw_state,
  withdraw_leaf
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

## Running The Bridge Fixture

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

## Legacy SP1 Template Notes

# SP1 Project Template

This is a template for creating an end-to-end [SP1](https://github.com/succinctlabs/sp1) project
that can generate a proof of any RISC-V program.

## Requirements

- [Rust](https://rustup.rs/)
- [SP1](https://docs.succinct.xyz/docs/sp1/getting-started/install)

## Running the Project

There are 3 main ways to run this project: execute a program, generate a core proof, and
generate an EVM-compatible proof.

### Build the Program

The program is automatically built through `script/build.rs` when the script is built.

### Execute the Program

To run the program without generating a proof:

```sh
cd script
cargo run --release -- --execute
```

Bridge proof

```sh
cd script
RUST_LOG=info cargo run --release --bin bridge  -- --execute
```

Withdraw proof

```sh
cd script
RUST_LOG=info cargo run --release --bin withdraw -- --execute
```

This will execute the program and display the output.

### Generate an SP1 Core Proof

To generate an SP1 [core proof](https://docs.succinct.xyz/docs/sp1/generating-proofs/proof-types#core-default) for your program:

```sh
cd script
cargo run --release -- --prove
```

### Generate an EVM-Compatible Proof

> [!WARNING]
> You will need at least 16GB RAM to generate a Groth16 or PLONK proof. View the [SP1 docs](https://docs.succinct.xyz/docs/sp1/getting-started/hardware-requirements#local-proving) for more information.

Generating a proof that is cheap to verify on the EVM (e.g. Groth16 or PLONK) is more intensive than generating a core proof.

To generate a Groth16 proof:

```sh
cd script
cargo run --release --bin evm -- --system groth16
```

To generate a PLONK proof:

```sh
cargo run --release --bin evm -- --system plonk
```

These commands will also generate fixtures that can be used to test the verification of SP1 proofs
inside Solidity.

### Retrieve the Verification Key

To retrieve your `programVKey` for your on-chain contract, run the following command in `script`:

```sh
cargo run --release --bin vkey
```

## Using the Prover Network

We highly recommend using the [Succinct Prover Network](https://docs.succinct.xyz/docs/network/introduction) for any non-trivial programs or benchmarking purposes. For more information, see the [key setup guide](https://docs.succinct.xyz/docs/network/developers/key-setup) to get started.

To get started, copy the example environment file:

```sh
cp .env.example .env
```

Then, set the `SP1_PROVER` environment variable to `network` and set the `NETWORK_PRIVATE_KEY`
environment variable to your whitelisted private key.

For example, to generate an EVM-compatible proof using the prover network, run the following
command:

```sh
SP1_PROVER=network NETWORK_PRIVATE_KEY=... cargo run --release --bin evm
```


### Update ark custom version

```
cargo update -p ark-ff 
cargo update -p ark-ec 
cargo update -p ark-poly 
cargo update -p ark-serialize 
RUST_LOG=info cargo run --release -- --execute
```


### Prover Network 

It takes less than 5 minutes to generate a Groth16 proof for 1B5 gas on the prover network.

The current cost to prove a Zeko rollup app command is around 1.1 PROVE tokens. The PROVE token price was $0.26 on May 7, so that comes to around $0.30.

[Request](https://explorer.succinct.xyz/request/0x67eecb92c7ed781f06271e661bcf49543eb2f555a98f80745e266e23d79b0b8a)
