# API and GraphQL

`zeko-proof-api` is both a Mina compatibility façade and the Ethereum proof
operator. The sequencer points its L1 and archive GraphQL URIs at this service;
users continue to send ordinary L2 transactions to the sequencer itself.

## Mina GraphQL subset

The gateway implements only operations used by
`src/app/zeko/sequencer/gql_client/gql_client.ml`:

| Operation | Backing data |
| --- | --- |
| `account` | Complete virtual Mina account JSON seeded at genesis and updated after confirmed settlements. |
| `pooledZkappCommands` | Pending settlement commands persisted by the gateway. |
| `pooledUserCommands` | Pending signed commands, when present. |
| `actions` | Exact proof-emitted outer actions indexed from confirmed Ethereum receipts. |
| `networkState` | Canonical/finalized Ethereum heights used by the Actions indexer. |
| `events` | Empty list; the current sequencer path does not require event data. |
| `genesisConstants` | Configured genesis timestamp and account-creation fee. |
| `runtimeConfig` | Configured fork slot. |
| `bestChain` | Canonical Ethereum block numbers and hashes in Mina-shaped fields. |
| `sendZkapp` | Queues an `EthereumSettlementInput` exported by the OCaml committer. |

Operations outside this subset return a GraphQL error. This is not a general
Mina node proxy.

### Settlement mutation authentication

The current OCaml client cannot attach a custom HTTP header, so `sendZkapp`
carries `gatewayToken` as a GraphQL variable. The sequencer reads it from
`ZEKO_ETHEREUM_GATEWAY_TOKEN`. Read queries remain unauthenticated.

The mutation is idempotent by the exported Mina transaction hash. Reusing a
hash for different proof input is rejected. The command remains visible in the
pending pool until the Ethereum transaction reaches consensus finality.

## REST API

### Operator routes

These routes require `x-api-key: <PROOF_API_KEY>`:

| Method and path | Purpose |
| --- | --- |
| `POST /v1/settlements` | Queue a settlement proof bundle. Alias: `/v1/proofs/settlement`. |
| `POST /v1/bridge/deposits/prove` | Queue the next canonical finalized native-deposit batch. |
| `POST /v1/proofs/bridge` | Low-level bridge fixture endpoint; not the production deposit entry point. |
| `POST /v1/proofs/withdraw` | Legacy withdrawal fixture endpoint. |
| `GET /v1/proofs` | List jobs, optionally filtered by kind/status. |
| `GET /v1/proofs/:id` | Inspect input digest, public values, costs, request and transaction state. |
| `GET /v1/proofs/:id/quote` | Read live auction parameters without creating a proof request. |
| `POST /v1/proofs/:id/approve` | Approve one exact preflight digest with PGU and price caps. |
| `POST /v1/proofs/:id/cancel` | Reject a job only before a network request exists. |

### Public bridge routes

| Method and path | Purpose |
| --- | --- |
| `GET /v1/bridge/config` | Chain, contract, decimal, finality-mode, and withdrawal-delay discovery for browser clients. |
| `GET /v1/bridge/deposits?zekoRecipient=0x...&after=N&limit=N` | Recover a wallet's deposits after a page reload. |
| `GET /v1/bridge/deposits/:nonce` | Deposit finality, proof, synchronization, and next user action. |
| `GET /v1/bridge/withdrawals?recipient=0x...&after=N` | Discover indexed native claims. |
| `GET /v1/bridge/withdrawals/:sequence/:offset` | Return one fixed-depth Merkle proof and live delay/cursor status. |
| `GET /health` | Database and Ethereum connectivity. |

## Exposure policy

Bind the gateway and sequencer to a private address. Put a TLS reverse proxy in
front of them and apply route-specific policy:

- `/graphql` serves Mina-compatible account/archive reads to the sequencer and
  bridge clients; settlement submission still requires `gatewayToken`.
- proof job, quote, approval, and cancellation routes require both network
  restriction and API-key authentication.
- bridge config, deposit/withdrawal discovery, and read-only GraphQL operations
  may be public and rate limited.
- PostgreSQL, RabbitMQ, DA RPC, and signer RPC must not be publicly reachable.

Do not expose the gateway directly to the Internet merely because it has an API
key. Its GraphQL handler uses a deliberately narrow operation recognizer, not a
full public GraphQL security layer.

`API_CORS_ALLOWED_ORIGINS` controls browser origins. Use an exact comma-separated
allowlist in a deployment; `*` is intended only for isolated development.

## Browser status model

Deposit responses expose both a stable `status` and `nextAction`. The status
progression is `confirming` → `locked` → `proofQueued`/`proving` →
`bridgeProven` → `synchronized`; approval mode inserts
`awaitingProofApproval`. A terminal proof error is `proofFailed`. Browser code
should display the server status and resume by nonce instead of keeping its own
authoritative state machine.

Withdrawal amounts are decimal strings, not JSON numbers. This preserves the
full Solidity/Mina `uint64` range in JavaScript.

## Virtual Mina state

`VIRTUAL_MINA_ACCOUNTS_PATH` seeds complete account objects for at least the
outer account and sequencer fee payer. `VIRTUAL_MINA_OUTER_PUBLIC_KEY` tells the
indexer where to publish bridge-produced actions.

Confirmed settlements update `zkappState`, the five-element action state,
fee-payer nonce, actions, block view, and pending pool atomically with a stored
pre-state snapshot. That snapshot is what makes reorg rollback deterministic.
