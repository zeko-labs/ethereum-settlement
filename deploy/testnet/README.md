# Persistent Sepolia PoC profile

This profile runs one gateway, one real OCaml sequencer/prover, and three
independent persistent DA nodes at quorum two. Only the gateway and sequencer
GraphQL ports may be published; PostgreSQL, RabbitMQ, DA RPC, and all signer
RPCs stay on Docker's internal network. Multisig DA is a PoC milestone, not the
production replacement for blob DA.

## Immutable inputs

Build the gateway from the exact exported OCaml VK and build the Zeko images
from the exact commit/config used to generate the fixtures. Push them, resolve
registry digests, then copy `.env.example` to `.env` and use only
`repo@sha256:<digest>` image references.

The Ethereum profile defaults `ZEKO_COMMITMENT_PERIOD_SECONDS` to 900. Keep it
explicit in the deployment environment so the explorer and operator runbook
match the running sequencer; changing it does not alter Mina deployment
defaults.

Generate the final bridge proxy address before building OCaml circuits. The
machine initializer creates a new admin first, predicts that admin's
deterministic proxy, and then generates the retained Zeko, DA,
bridge-recipient, gateway, upgrader, and proof-network identities around it:

```sh
tools/init-machine-testnet-identity.sh deploy/testnet
```

This command refuses to overwrite an existing identity, writes secrets with
mode `0600`, creates the circuit/deploy configurations bound to the final
bridge address, and records the public sequencer, DA, gateway, requester, and
bridge-recipient identities. Keep the resulting directory encrypted and
backed up. Use those exact identities for fixture export and the persistent
deployment; a fixture produced with disposable test keys cannot bootstrap this
profile.

Generate the real two-commit fixture set with the retained environment:

```sh
set -a
source deploy/testnet/secrets/fixture-keys.env
set +a
POC_ENV_FILE=deploy/testnet/secrets/fixture-keys.env \
  tools/export-bridge-ocaml-fixtures.sh build/poc/testnet-bridge-fixtures
```

The fixture environment sets the 2400-slot validity period used with 12-second
Sepolia slots. The runtime profile also passes that 12-second duration to the
sequencer so its computed commit range tracks the settlement contract. It pins
both circuits and signers to the exact Mina `testnet` domain required by Auro
in this PoC. The export records the public keys and exact genesis ledger.

Populate `config/` as described in [config/README.md](config/README.md). The
virtual Mina accounts file must include the outer account and settlement fee
payer from `deposit-sync/settlement.json`, with the initial action state from
`bridge-scenario.json.outerActionStateBeforeDeposit`.

After `prepare-poc.sh` and contract deployment, set the deployment block and
hard caps, then materialize the non-secret profile files:

```sh
ETHEREUM_INDEXER_START_BLOCK=<deployment-block> \
PROVER_GAS_LIMIT=<hard-pgu-cap> \
PROVER_MAX_PRICE_PER_PGU=<hard-price-cap> \
tools/materialize-testnet-config.sh \
  build/poc/testnet-bridge-fixtures \
  deploy/testnet/config/circuits.json build/poc-sepolia
```

## Secrets

Create these newline-terminated files with mode `0600`:

```text
proof-api-key
actions-indexer-token
network-private-key
admin-private-key
upgrader-private-key
deployment-roles.env
settlement-private-key
bridge-private-key
withdraw-private-key
postgres-gateway-password
postgres-sequencer-password
postgres-explorer-password
rabbitmq-password
sequencer-private-key
sequencer-signer-token
da1-private-key
da1-signer-token
da2-private-key
da2-signer-token
da3-private-key
da3-signer-token
bridge-recipient-private-key
signer-tls.crt
signer-tls.key
```

`init-machine-testnet-identity.sh` also creates the private Zeko deploy config
and fixture-only environment file. Neither is mounted into the running gateway
or published as an artifact. The TLS certificate must have SAN entries for `sequencer-signer`,
`da1-signer`, `da2-signer`, and `da3-signer`. Use distinct random signer auth
tokens. Admin, upgrader, gateway prover, and Succinct requester identities must
be distinct; preflight rejects role sharing. Admin/upgrader custody should
remain outside the long-running runtime host.

## Build and deployment helpers

After preparing the Sepolia manifest against the retained fixture, build and
pin the machine images:

```sh
tools/build-machine-images.sh \
  build/poc/testnet-bridge-fixtures/deposit-sync/vk.serde.json deploy/testnet
```

The script runs a loopback registry and writes `artifacts/images.json` plus
immutable image references in `.env`. Contract deployment requires an explicit
write acknowledgement:

```sh
CONFIRM_SEPOLIA_DEPLOY=yes \
  tools/deploy-machine-poc.sh build/poc-sepolia deploy/testnet
```

Copy `gateway.env.example` to `gateway.env` and fill the Sepolia RPC, deployed
contracts, genesis timestamp/fork slot, outer public key, and indexer start
block. The Compose profile overrides all bypass modes and forces
`API_REQUIRE_PROOF_APPROVAL=true`.

`archive-db-reader` creates a dedicated `zeko_explorer` PostgreSQL role with
read-only defaults, a five-second statement timeout, and `SELECT` privileges
on current and future archive tables. Use a URL-safe random value for
`postgres-explorer-password`; the gateway never receives the sequencer's
owner password. On first bootstrap the gateway may start before the sequencer
has created the archive tables. It reports the archive source as unavailable
until schema creation completes, then polling explorer clients recover without
a restart.

## Bootstrap and start

After deploying the contracts and preparing the configuration, run:

```sh
tools/testnet-preflight.sh deploy/testnet
docker compose --env-file deploy/testnet/.env \
  -f deploy/testnet/compose.yaml up -d

tools/machine-actions-services.sh start deploy/testnet
```

`bootstrap-da` is a bounded, idempotent one-shot. It reconstructs the deploy
command from the exported genesis ledger and posts that ledger to all three DA
nodes. The sequencer starts only after bootstrap succeeds. On later restarts,
the Zeko deploy helper checks that the same ledger already exists and exits
without changing it.

An empty replacement DA node can copy the retained chain from a trusted peer.
Set, for example, `DA2_RESTORE_FROM_PEER=da1:5000` before starting DA2. If the
peer has more than one uncommitted head, also set
`DA2_RESTORE_TARGET_LEDGER_HASH=<decimal-ledger-hash>`. Restore is skipped when
the local database already contains diffs. The node checks chain continuity but
does not independently recompute ledger roots, so snapshots and peer selection
remain operator trust decisions.

`prover-ready` is also a bounded one-shot. It waits until RabbitMQ reports an
OCaml prover consumer, which means the real circuit compilation has completed;
the sequencer is not allowed to initialize against a half-started prover.

Do not expose the gateway directly to the Internet. Put an authenticated,
rate-limited TLS reverse proxy in front of `/graphql` and the proof-operator
routes; the public bridge discovery endpoints may be separately exposed.

The Actions services run from the exact clean `ZEKO_UI_ROOT` and
`ZEKO_UI_COMMIT` recorded in `.env`. Serve the separate React application from
`bridge-ui/dist` and provide its public `runtime-config.json`; never put a proof
API token or runtime private key in that static file.

Serve the L2 explorer independently from `explorer-ui/dist`, with its public
`runtime-config.json` pointing at the public gateway and bridge UI. Only the
gateway's `/v1/explorer/*` routes should be exposed to it; the browser never
connects to PostgreSQL and never calls proof-operator routes.

## Proof runbook

Every bridge or settlement job is verified locally and stops in
`awaiting_approval`. Settlement verification uses the pinned native Pickles
path; bridge validation executes its guest. For each of the three paid
boundaries in the demo:

1. Inspect the job public values and live contract state.
2. Call `GET /v1/proofs/:id/quote` with a simulation-derived PGU cap.
3. Record the digest, quote, balance, and operator decision in the run
   artifacts.
4. Approve the exact preflight digest with explicit `maxPgu` and
   `maxPricePerPgu` strings.
5. Wait until the transaction block is reported by Sepolia's JSON-RPC
   `finalized` tag before advancing the next state writer.

The expected round trip is one bridge proof, one deposit-synchronizing OCaml
settlement, and one withdrawal-bearing OCaml settlement. Deposit finalization
also requires the inner account's no-settlement synchronization pass; the
sequencer performs it as OCaml state progression and it does not create a
fourth Ethereum proof.

Stop immediately on a vkey/address mismatch, expired slot window, DA quorum
loss, reorg, or quote above the approved budget. Never switch approval mode off
to bypass a stuck job.
