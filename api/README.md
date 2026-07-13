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
- `POST /v1/bridge/deposits/prove` — builds a native-deposit proof job from
  the next contiguous finalized `BridgeDeposit` logs; callers cannot supply
  deposit contents
- `POST /v1/proofs/withdraw`
- `GET /v1/bridge/withdrawals/:sequence/:offset` — returns the ordinary
  Keccak Merkle proof and claim data for a settlement-bound native withdrawal
- `GET /v1/proofs` and `GET /v1/proofs/:id`
- `GET /health`

All `/v1` routes require `x-api-key`. Mutations are idempotent and a Mina
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

Locally:

```sh
createdb zeko_proofs
set -a; source .env.api; set +a
cargo run --release -p zeko-proof-api
```

`VIRTUAL_MINA_ACCOUNTS_PATH` points to a JSON array of complete Mina GraphQL
account objects for the outer account and fee payer. Existing rows are not
overwritten at startup.

`VIRTUAL_MINA_OUTER_PUBLIC_KEY` identifies the rollup outer account updated by
confirmed bridge receipts. The indexer decodes every exact five-field outer
Witness action from the SP1 receipt, checks each intermediate Poseidon action
state, and exposes those same fields through Mina-compatible `actions` reads.

For the native bridge PoC, the gateway also indexes canonical bridge deposits
and settlement V2 inner-action batches. Deposit proof jobs are constructed only
from contiguous finalized logs beginning at the bridge contract's proven
nonce. Settlement confirmation stores the ordered inner-action leaves. The
public withdrawal endpoint returns a depth-16 Keccak proof, so a user can claim
on Ethereum without generating a Mina or SP1 proof.

The worker records `cycleCount`, `proverGas`, the network base/max prices,
actual PROVE deduction after refund, Ethereum gas, confirmations, and explorer
URL when those values are available.
