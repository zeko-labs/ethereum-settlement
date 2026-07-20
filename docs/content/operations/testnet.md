# Sepolia testnet runbook

This runbook deploys the no-blob PoC: one real sequencer/prover, three retained
DA nodes at quorum two, the gateway, PostgreSQL, RabbitMQ, isolated signers, and
the Ethereum settlement/bridge contracts.

The pinned Compose reference lives in `deploy/testnet`. For a NixOS deployment,
implement the same topology and invariants described in the [DevOps guide](/operations/devops).

## 1. Freeze the release identity

The following values must be chosen together and never mixed across builds:

- final deterministic bridge proxy address
- OCaml circuit configuration containing that address
- sequencer, three DA, and bridge-recipient public identities
- exact genesis ledger and two-commit bridge scenario
- OCaml wrap verifier index and its PoC SHA-256 identifier
- settlement, bridge, and withdrawal SP1 program vkeys
- settlement/bridge proxy addresses and Sepolia chain ID

`build/poc/manifest.json`, and later
`deploy/testnet/artifacts/manifest.json`, is the public identity record. Review
and archive it with the build provenance.

## 2. Initialize retained secrets

Create the complete machine-local release identity in one step:

```sh
tools/init-machine-testnet-identity.sh deploy/testnet
```

The initializer creates distinct admin, upgrader, gateway prover, and Succinct
requester identities. It predicts the deterministic proxy from the final admin,
then generates the OCaml circuit/deploy configuration, sequencer identity,
three DA identities, bridge recipient, signer credentials, and public identity
record around that address. Prediction does not deploy anything.

The script refuses to overwrite identities and writes private files with mode
`0600`. Back up the secret directory in an encrypted system. The generated
fixture environment is build-time only; do not mount it into the running
gateway.

Use distinct Mina keys for the sequencer and each DA signer. The current PoC
uses one Ethereum gateway prover address for all three submitter files because
the preflight enforces that identity. Admin, upgrader, gateway prover, and
network requester are distinct; only the gateway prover key belongs on the
runtime host.

## 3. Generate genuine OCaml artifacts

Load the retained fixture keys and generate the bridge scenario:

```sh
set -a
source deploy/testnet/secrets/fixture-keys.env
set +a

POC_ENV_FILE=deploy/testnet/secrets/fixture-keys.env \
  tools/export-bridge-ocaml-fixtures.sh build/poc/testnet-bridge-fixtures
```

That retained environment fixes both OCaml chains and the external signer to
Mina's exact `testnet` signing domain for Auro compatibility.

The testnet scenario uses a 2400-slot commit validity period with 12-second
Sepolia slots. `ZEKO_SLOT_DURATION_SECONDS=12` keeps the sequencer's current
slot calculation aligned with `ZekoSettlement.slotDuration`. It must export two
chained settlements under one VK, three distinct DA keys, a synchronized native
deposit, and one bound native withdrawal preimage.

## 4. Prepare and build immutable images

With a Sepolia RPC URL and the public retained role addresses loaded, prepare
the deterministic deployment identity. This computes SP1 program vkeys and
builds the gateway against the exact retained OCaml verifier index; it does not
prove or deploy:

```sh
set -a
source deploy/testnet/secrets/deployment-roles.env
set +a

tools/prepare-poc.sh "$RPC_URL" "$ADMIN_ADDRESS" \
  build/poc/testnet-bridge-fixtures/deposit-sync build/poc-sepolia
```

Then build the gateway and OCaml runtime images from clean, committed source
revisions and pin them in the machine-local loopback registry:

```sh
tools/build-machine-images.sh \
  build/poc/testnet-bridge-fixtures/deposit-sync/vk.serde.json deploy/testnet
```

The command writes `deploy/testnet/artifacts/images.json`, updates `.env` with
`repo@sha256` references, and records both source revisions and the embedded VK
digest. Tags and `latest` are rejected by preflight.

The gateway recomputes all embedded program vkeys at startup and exits if they
do not match the live contracts.

## 5. Deploy Ethereum contracts

Use a real SP1 6.1-compatible verifier, then deploy and atomically initialize
the deterministic `ZekoSettlement` and `EthereumZekoBridge` proxies. Initialize:

- all eight outer-state fields
- initial outer action state and length
- genesis timestamp, 12-second slot duration, and fork slot
- settlement VK identifier and all SP1 program vkeys
- bridge proxy link and five-slot PoC withdrawal delay

Grant the gateway prover address `PROVER_ROLE` and the upgrader address
`UPGRADER_ROLE` on both contracts. The bridge proxy receives `BRIDGE_ROLE` on
settlement through `setBridgeContract`. The deployment script revokes prover
and upgrader roles from admin; preflight verifies the complete role matrix.

Fund admin for the one-time deployment, the gateway prover with Sepolia ETH,
and the Succinct requester with enough PROVE for three capped requests plus
retry margin. Deployment is an explicit write boundary:

```sh
CONFIRM_SEPOLIA_DEPLOY=yes \
  tools/deploy-machine-poc.sh build/poc-sepolia deploy/testnet
```

## 6. Materialize runtime config

Copy `deploy/testnet/.env.example` and `gateway.env.example`, then populate the
read-only `config/` directory with the exact circuits file, genesis ledger,
scenario manifest, and complete virtual Mina accounts.

After contract deployment:

```sh
ETHEREUM_INDEXER_START_BLOCK=<deployment-block> \
PROVER_GAS_LIMIT=<hard-pgu-cap> \
PROVER_MAX_PRICE_PER_PGU=<hard-price-cap> \
tools/materialize-testnet-config.sh \
  build/poc/testnet-bridge-fixtures \
  deploy/testnet/config/circuits.json build/poc-sepolia
```

Set the indexer start block to the deployment block, not the current head. The
outer and fee-payer virtual accounts must match the first settlement's genesis
state and nonce.

## 7. Preflight and start

```sh
tools/testnet-preflight.sh deploy/testnet

docker compose --env-file deploy/testnet/.env \
  -f deploy/testnet/compose.yaml up -d

tools/machine-actions-services.sh start deploy/testnet
```

Preflight rejects mutable image tags, non-Sepolia RPC, bypass modes, missing
price caps, bad secret permissions, wrong roles/vkeys/addresses, mismatched
identities, non-2-of-3 DA, and invalid Compose.

`bootstrap-da` posts the retained genesis ledger idempotently. `prover-ready`
waits for a RabbitMQ consumer before the sequencer starts, avoiding
initialization against an OCaml prover that is still compiling circuits.

The Actions indexer and API intentionally run from the separately pinned,
clean Zeko UI checkout. They use a dedicated database and expose only the
public witness-preparation surface required by the browser application.

Build `bridge-ui/`, materialize its public `runtime-config.json`, and serve the
static `dist/` directory behind TLS. Allow that origin through the gateway,
sequencer, and Actions CORS policies. See [bridge web application](/bridge-ui).

## 8. Execute before paying

Before enabling paid testnet operation, replay every genuine input through an
equivalent gateway built with `API_EXECUTE_ONLY=true` and compare its public
values with the initialized contracts.

The persistent profile forces approval mode. For each of the three demo jobs:

1. wait for `awaiting_approval`
2. inspect public values and live contract preconditions
3. obtain a read-only quote with a simulation-derived PGU cap
4. archive the digest, quote, balances, and approval decision
5. approve the exact digest with explicit PGU and price caps
6. wait until the transaction block is at or below the Sepolia JSON-RPC
   `finalized` head before advancing the next state writer

The order is bridge proof, deposit-synchronizing settlement, then
withdrawal-bearing settlement.

## 9. Acceptance transaction

Run one native round trip with a deliberately small amount:

- deposit ETH and observe canonical finality
- request the bridge batch proof and confirm its outer action
- wait for a genuine settlement to synchronize the deposit
- sign and submit `finalizeDeposit` on Zeko
- sign and submit a native withdrawal on Zeko
- wait for the V2 settlement and configured withdrawal delay
- retrieve the public path and claim on Ethereum
- reconcile user balances, bridge balance/liability, settlement states, DA
  availability, proof costs, Ethereum gas, and recipient cursor

Stop immediately on any vkey/address mismatch, expired slot window, DA quorum
loss, reorg, price above cap, public-values mismatch, or liability mismatch. Do
not disable approval mode to work around a stuck job.
