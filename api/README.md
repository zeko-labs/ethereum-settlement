# Zeko Ethereum Gateway

The gateway is the compatibility and proving service between the OCaml Zeko
sequencer and Ethereum. It exposes the subset of Mina GraphQL used by
`gql_client.ml`, validates an OCaml proof export with the pinned native Pickles
verifier and shared receipt derivation, requests the EVM proof from the Succinct
Network, submits it to Ethereum, and waits for configurable finality.

## Interfaces

- `POST /graphql` — Mina-compatible reads plus the gateway `sendZkapp`
  extension. Read operations are public. The mutation carries
  `gatewayToken` because the current OCaml client cannot attach a custom HTTP
  header.
- `POST /v1/settlements` and `POST /v1/proofs/settlement`
- `POST /v1/settlement-reservations` — reserve an outer checkpoint before
  synchronizing the OCaml inner account or preparing its outer proof
- `POST /v1/settlement-reservations/:id/renew` and
  `DELETE /v1/settlement-reservations/:id` — renew or release preparation
- `GET /v1/settlements/by-mina-hash/:hash` — explicit pending, failed or
  finalized outcome with the source and target ledger hashes
- `POST /v1/proofs/bridge`
- `POST /v1/bridge/deposits/prove` — builds a deposit proof job from the next
  contiguous finalized `BridgeDeposit` logs; callers cannot supply deposit
  contents
- `GET /v1/bridge/deposits/:nonce` — reports immutable action encoding and
  registry identity, Ethereum finality, the exact bridge-proved outer action,
  synchronization, and the next user action
- `GET /v1/bridge/withdrawals?recipient=0x...&after=<index>` — discovers
  settlement-bound native withdrawals and returns their ordinary Merkle paths
- `GET /v1/bridge/withdrawals/:sequence/:offset` — returns the ordinary
  Keccak Merkle proof, delay/cursor status, and claim data for one withdrawal
- `GET /v1/bridge/token-withdrawals/:sequence/:offset` — returns the
  registry-bound ERC-20 claim identity, proof, delay/cursor status, and claim
  data
- `GET /v1/proofs` and `GET /v1/proofs/:id`
- `GET /v1/proofs/:id/quote` — reads current auction parameters without
  creating a proof request
- `POST /v1/proofs/:id/approve` — approves one preflight digest with explicit
  PGU and price caps
- `POST /v1/proofs/:id/cancel` — rejects a job only while no network request
  or signed Ethereum transaction exists
- `GET /health`

Proof-job and proof-creation routes require `x-api-key`; bridge discovery and
withdrawal Merkle proofs are public. Mutations are idempotent and a Mina
transaction hash cannot be reused for different input. One durable reservation
owns outer-state preparation and the accepted job through finality. Successor
OCaml batches can remain in the sequencer's durable queue; they acquire the
next checkpoint after the preceding settlement finishes.

Bridge batches and settlements are mutually exclusive outer action-state
writers. GraphQL, REST, manual bridge and automatic bridge admission use the
same database lock. A bridge batch cannot enter while settlement preparation
owns the reservation. A settlement supplies `reservation: {id, fencingToken}`
from the acquisition response, and its immutable proof binding must match the
reserved checkpoint. The preparation lease defaults to 120 seconds and is
renewable; once attached to a job, ownership does not expire while proving or
waiting for finality. A stale fencing token cannot admit work after expiry.

Transient RPC errors and insufficient funds retain the pending command, proof
request ID, completed proof and any signed transaction. The worker retries the
same stage with capped backoff. A signed transaction's hash is reconciled before
rebroadcasting identical bytes; ambiguous sends retain ownership. An unsigned
stale checkpoint is terminal so the sequencer can rebuild its outer proof from
the saved witness. Gateway replicas use a PostgreSQL session lock to elect one
proof worker, and only that worker recovers interrupted stages.

Before asking SP1 to create a proof, the worker records a unique request intent.
If it loses the response before recording the request ID, the job reports
`proof_request_ambiguous` and retains its reservation. It cannot automatically
buy another proof or be cancelled as unsubmitted work. A matching late response
resumes the saved request; otherwise an operator must reconcile that intent
with the proving service before retrying. Existing request IDs and completed
proofs always resume without repeating proof creation.

Deploy the gateway and matching sequencer together after draining active jobs.
Migrations 0020 and 0021 add reservations and durable submission stages; the
new gateway rejects settlement admission without a reservation. Keep both
databases and the sequencer ledger intact during rollout. Check the sequencer's
`/readyz` and the explicit settlement outcome before enabling traffic; pool
disappearance alone is not proof of finality.

The Mina compatibility subset is deliberately narrow:

- `account`
- `pooledZkappCommands` and `pooledUserCommands`
- `actions` and empty `events`
- `genesisConstants`, `runtimeConfig`, and `bestChain`
- `sendZkapp` with `EthereumSettlementInput`

Confirmed settlements update `zkappState`, the rolling five-element action
state, fee-payer nonce, and action rows. Pending commands are removed only
after finality. The indexer records canonical blocks and account snapshots so
an Ethereum reorg restores the prior virtual Mina view and requeues the same
proof rather than purchasing a second proof.

`ETHEREUM_FINALITY_MODE` defaults to `finalized`. In that mode the gateway
reads the JSON-RPC `finalized` block, verifies its hash against the locally
indexed canonical chain, and exposes only finalized actions. An unsupported or
inconsistent finalized checkpoint fails closed. The `confirmations` mode and
`ETHEREUM_CONFIRMATIONS` depth are runtime-restricted to local chain ID 31337;
testnet preflight also rejects them.

Receipt polling stops once a job is consensus-finalized, its inclusion block
matches the indexed canonical chain, and its virtual state has been applied.
Confirmed historical jobs do not consume more RPC calls or change their
completion timestamp on each indexer pass. Local confirmation mode keeps
rechecking confirmed receipts because a depth threshold remains reversible.
Reverted transactions become terminal only after canonical finality as well.
The indexer checks the canonical tip even when its height stays unchanged or
regresses, so local snapshot reverts restore virtual accounts without waiting
for another block. It never rolls back a consensus-finalized checkpoint.
Deposit logs are requested by block hash and committed together with the block
cursor; exhausted RPC retries cannot advance past an unread deposit block.

All gateway Ethereum calls, including transaction wallet fillers, share one
HTTP connection pool, request budget and concurrency limit per process. The
defaults in `.env.api.example` are:

| Variable | Default | Meaning |
| --- | --- | --- |
| `ETHEREUM_RPC_REQUESTS_PER_SECOND` | `8` | RPC calls per second; batch members each consume budget |
| `ETHEREUM_RPC_MAX_CONCURRENT` | `4` | Maximum HTTP requests in flight |
| `ETHEREUM_RPC_MAX_RETRIES` | `5` | Additional attempts for transient read failures |
| `ETHEREUM_RPC_INITIAL_BACKOFF_MS` | `500` | Initial exponential backoff with jitter |
| `ETHEREUM_RPC_MAX_BACKOFF_MS` | `10000` | Maximum generated backoff; server hints may be longer |
| `ETHEREUM_RPC_REQUEST_TIMEOUT_MS` | `30000` | Timeout for one HTTP request |
| `ETHEREUM_RPC_OPERATION_TIMEOUT_MS` | `90000` | Total bounded time including pacing and retries |

HTTP 429, temporary HTTP failures and recognized JSON-RPC throttling responses
retry safe reads. `Retry-After` and provider backoff hints delay every clone of
the client. Contract reverts are returned immediately. Size each gateway's
budget to fit the RPC account allowance shared with other instances and tools.

Settlement, bridge and token-identity snapshots pin related contract reads to
one canonical block hash using EIP-1898. The gateway requires an RPC endpoint
that serves these calls; it does not mix fields from different `latest` blocks.
Transaction preparation fills and signs without sending. The worker persists
the signed bytes and hash before broadcasting; a retry first reconciles that
hash and only resends the identical transaction. Raw sends are never replayed
by the read retry transport.

## Settlement input

The OCaml exporter supplies the four Pickles files plus a `binding` object:

```json
{
  "schemaVersion": 1,
  "minaTransactionHash": "0x<32 bytes>",
  "proof": {
    "vkJson": "...",
    "proofJson": "...",
    "publicInputSkeletonJson": "...",
    "appStatementJson": "[\"0x...\",\"0x...\"]",
    "binding": {
      "minaSignatureKind": "testnet",
      "accountUpdateBody": {
        "fieldElements": ["0x..."],
        "packed": [{"value": "0x...", "bits": 1}]
      },
      "actions": [["0x...", "0x..."]],
      "stateBefore": {"fields": ["0x...", "0x...", "0x...", "0x...", "0x...", "0x...", "0x...", "0x..."]}
    }
  }
}
```

When the job reaches the worker, the gateway adds `proof.context` from the live
contract (chain ID, contract address, next batch, outer action length and
transaction hash). SP1 derives the receipt; neither the API nor the sequencer
supplies a trusted `stateAfter`.

## Running

```sh
cp .env.api.example .env.api
docker compose up --build -d
curl http://127.0.0.1:8080/health
```

For execute-only validation with no network proof and no Ethereum write:

```sh
API_EXECUTE_ONLY=true docker compose up --build -d
```

For a full local contract transition after the same SP1 execution, deploy the
contracts with `LOCAL_MOCK_VERIFIER=true` and run the gateway with:

```sh
API_LOCAL_MOCK_SUBMIT=true docker compose up --build -d
```

This mode submits the preflight public values with an empty proof. Startup
fails unless every configured verifier is the repository's
`LocalSP1Verifier`. It is restricted to chain ID `31337` unless the explicitly
insecure `API_UNSAFE_ALLOW_MOCK_ON_SEPOLIA=true` PoC override is also set on
Sepolia. It is mutually exclusive with `API_EXECUTE_ONLY` and
`API_REQUIRE_PROOF_APPROVAL`; no Succinct proof request is created.

Locally:

```sh
createdb zeko_proofs
set -a; source .env.api; set +a
cargo run --release -p zeko-proof-api
```

At startup the gateway derives both vkeys from its embedded ELFs and
compares them with the settlement and bridge contracts. A binary built against
the wrong OCaml settlement VK therefore exits before accepting jobs. Use
`tools/prepare-poc.sh` to build the gateway and deployment manifest from one
fixture identity.

`VIRTUAL_MINA_ACCOUNTS_PATH` points to a JSON array of complete Mina GraphQL
account objects for the outer account and fee payer. Existing rows are not
overwritten at startup.

`VIRTUAL_MINA_OUTER_PUBLIC_KEY` identifies the rollup outer account updated by
confirmed bridge receipts. The indexer decodes every exact five-field outer
Witness action from the SP1 receipt, checks each intermediate Poseidon action
state, and exposes those same fields through Mina-compatible `actions` reads.

The gateway indexes canonical native and ERC-20 bridge deposits and settlement
V2/V3/V4 inner-action batches. Deposit proof jobs are constructed only from
contiguous finalized logs beginning at the bridge contract's proven nonce. A
confirmed bridge receipt binds each deposit nonce to its exact outer action
sequence; a later settlement marks only the covered synchronized sequences.
Settlement confirmation also stores the ordered inner-action leaves and
immutable registry identity. The public withdrawal endpoints return depth-16
Keccak proofs and read the live virtual slot plus per-recipient cursor, so a
user can discover and claim on Ethereum without generating a Mina or SP1
proof.

The worker records `cycleCount` when a local zkVM execution occurred,
`proverGas`, the network base/max prices, actual PROVE deduction after refund,
Ethereum gas, confirmations, and explorer URL when those values are available.
Operational settlement validation is native, so its cycle count stays null
until the network reports metrics.

## Paid proof approval

Persistent testnet deployments should set `API_REQUIRE_PROOF_APPROVAL=true`.
Every job then completes its local preflight and pauses in
`awaiting_approval`; settlement uses pinned native Pickles verification while
bridge uses the zkVM executor. The worker cannot call the Succinct
network from that state. Inspect the job and read a quote:

```sh
curl -H "x-api-key: $PROOF_API_KEY" \
  "$GATEWAY_URL/v1/proofs/$JOB_ID"
curl -H "x-api-key: $PROOF_API_KEY" \
  "$GATEWAY_URL/v1/proofs/$JOB_ID/quote?maxPgu=$MAX_PGU&maxPricePerPgu=$MAX_PRICE"
```

Approve the exact `preflightInputDigest` returned by the job. Numeric caps are
strings so JSON clients cannot round 64-bit values:

```sh
jq -n --arg digest "$PREFLIGHT_INPUT_DIGEST" \
  --arg maxPgu "$MAX_PGU" --arg maxPricePerPgu "$MAX_PRICE" \
  '{inputDigest:$digest,maxPgu:$maxPgu,maxPricePerPgu:$maxPricePerPgu}' \
| curl -H "x-api-key: $PROOF_API_KEY" -H 'content-type: application/json' \
    --data-binary @- "$GATEWAY_URL/v1/proofs/$JOB_ID/approve"
```

Approval revalidates the persisted public values against live contracts and,
for settlements, requires at least `PROVER_MIN_REMAINING_SLOTS` before the
proof-bound upper slot. `PROVER_GAS_LIMIT` and
`PROVER_MAX_PRICE_PER_PGU` remain deployment-wide hard ceilings; a per-job
approval can only be tighter. The approval response snapshots a read-only
auction quote, but it still does not create the paid request. The worker does
that only after atomically claiming the `approved` job.
