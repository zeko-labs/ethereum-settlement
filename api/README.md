# Zeko Proof API

Asynchronous REST API for validating inputs, requesting SP1 Network proofs, and
submitting them to Ethereum.

## Endpoints

- `POST /v1/proofs/settlement`
- `POST /v1/proofs/bridge`
- `POST /v1/proofs/withdraw`
- `GET /v1/proofs?kind=bridge&status=confirmed&limit=50`
- `GET /v1/proofs/:id`
- `GET /health`

All `/v1` endpoints require `x-api-key`. `POST` requests may also send an
`Idempotency-Key` header. A submission returns HTTP `202`; poll the returned
status URL until the job is `confirmed` or `failed`.

```sh
curl -X POST http://127.0.0.1:8080/v1/proofs/bridge \
  -H "x-api-key: $PROOF_API_KEY" \
  -H "Idempotency-Key: bridge-batch-42" \
  -H "content-type: application/json" \
  --data-binary @proofs/bridge-input.json
```

Settlement requests contain the GraphQL mutation. They may also include the
expected on-chain state for an additional cheap check before local SP1
execution:

```json
{
  "graphql": "mutation { sendZkapp(input: ...) { ... } }",
  "expected": {
    "vk_hash": "0x...",
    "action_state": "0x...",
    "current_root": "0x..."
  }
}
```

Bridge and withdraw bodies use `BridgeTransitionInput` and
`WithdrawTransitionInput` JSON directly. See `proofs/bridge-input.json` and
`proofs/withdraw-input.json`.

## Run

```sh
createdb zeko_proofs
cp .env.api.example .env.api
set -a; source .env.api; set +a
cargo run --release -p zeko-proof-api
```

The API stores job inputs and results in PostgreSQL. Ethereum and SP1 private
keys are read only by the worker process from environment variables.

The worker directly uses the SP1 SDK and Alloy. It does not invoke shell
scripts, `cargo`, or `cast`.

Before requesting a paid proof, it executes the SP1 program locally and checks
the resulting public values against Ethereum. SP1 request IDs are persisted
immediately, allowing interrupted jobs to resume the existing network request
after a restart.
