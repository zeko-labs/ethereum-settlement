# Zeko Ethereum Gateway

The gateway is the compatibility and proving service between the OCaml Zeko
sequencer and Ethereum. It exposes the subset of Mina GraphQL used by
`gql_client.ml`, validates an OCaml proof export by executing SP1 locally,
requests the EVM proof from the Succinct Network, submits it to Ethereum, and
waits for configurable finality.

## Interfaces

- `POST /graphql` — Mina-compatible reads plus the gateway `sendZkapp`
  extension. Read operations are public. The mutation carries
  `gatewayToken` because the current OCaml client cannot attach a custom HTTP
  header.
- `POST /v1/settlements` and `POST /v1/proofs/settlement`
- `POST /v1/proofs/bridge`
- `POST /v1/bridge/deposits/prove` — builds a deposit proof job from the next
  contiguous finalized `BridgeDeposit` logs; callers cannot supply deposit
  contents
- `POST /v1/proofs/withdraw`
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
  exists
- `GET /health`

Proof-job and proof-creation routes require `x-api-key`; bridge discovery and
withdrawal Merkle proofs are public. Mutations are idempotent and a Mina
transaction hash cannot be reused for different input. Multiple OCaml commits
may queue, but only one settlement can be proving or submitted at a time. The
gateway assigns its Ethereum batch/action context only when it reaches the
worker, after the previous settlement is confirmed.

Bridge batches and settlements are mutually exclusive outer action-state
writers. A bridge batch is rejected while any settlement is queued or active,
and a settlement is rejected while a bridge batch is queued or active. This
prevents purchasing two proofs against the same starting action checkpoint.

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
`LocalSP1Verifier` and the chain ID is exactly `31337`. It is mutually
exclusive with `API_EXECUTE_ONLY` and cannot be used for testnet proving.

Locally:

```sh
createdb zeko_proofs
set -a; source .env.api; set +a
cargo run --release -p zeko-proof-api
```

At startup the gateway derives all three vkeys from its embedded ELFs and
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
bridge/withdraw use the zkVM executor. The worker cannot call the Succinct
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
