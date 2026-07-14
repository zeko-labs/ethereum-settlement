# Proof jobs and approval

The gateway owns all SP1 work. The sequencer and bridge users never hold a
Succinct requester key and never generate a proof themselves.

## Job lifecycle

In the testnet profile, a new job follows:

```text
queued -> validating -> awaiting_approval -> approved
  -> proving / proof_requested -> submitting -> submitted -> confirmed
```

`validating` runs the complete SP1 guest locally through the low-memory direct
executor. It records public values and cycle count, validates them against live
contract state, and hashes the hydrated proof input. No paid request exists at
this point.

Other terminal or recovery states include `executed` (execute-only), `rejected`,
`failed`, `proof_failed`, `ethereum_reverted`, and `reorged`.

## Approval boundary

Persistent deployments set:

```text
API_REQUIRE_PROOF_APPROVAL=true
PROOF_SYSTEM=groth16
```

Inspect the job and obtain a read-only quote:

```sh
curl -H "x-api-key: $PROOF_API_KEY" \
  "$GATEWAY_URL/v1/proofs/$JOB_ID"

curl -H "x-api-key: $PROOF_API_KEY" \
  "$GATEWAY_URL/v1/proofs/$JOB_ID/quote?maxPgu=$MAX_PGU&maxPricePerPgu=$MAX_PRICE"
```

Executor cycles are not network PGU. Supply a PGU cap from an SP1 simulation
or a deliberately conservative operator budget. The quote reads current
auction base fee and maximum price; it does not register a program or create a
proof request.

Approve the exact `preflightInputDigest`:

```sh
jq -n \
  --arg digest "$PREFLIGHT_INPUT_DIGEST" \
  --arg maxPgu "$MAX_PGU" \
  --arg maxPrice "$MAX_PRICE" \
  '{inputDigest:$digest,maxPgu:$maxPgu,maxPricePerPgu:$maxPrice}' \
| curl -H "x-api-key: $PROOF_API_KEY" \
    -H 'content-type: application/json' \
    --data-binary @- \
    "$GATEWAY_URL/v1/proofs/$JOB_ID/approve"
```

Caps are JSON strings to avoid 64-bit rounding in clients. Per-job caps may
only tighten the deployment-wide `PROVER_GAS_LIMIT` and
`PROVER_MAX_PRICE_PER_PGU` ceilings.

Approval re-reads live contract state and, for settlements, requires at least
`PROVER_MIN_REMAINING_SLOTS` before the proof-bound upper slot. The worker
creates the paid request only after atomically claiming the approved job.

## Cost accounting

When available, each job records:

- local execution cycles
- network prover gas (PGU)
- base fee and maximum price per PGU
- actual PROVE deduction after refund
- Ethereum gas used
- confirmations and explorer URLs

Network prices are live auction values. There is intentionally no static
currency estimate in the docs.

The expected native round trip uses three paid proofs: one deposit bridge proof,
one deposit-synchronizing settlement, and one withdrawal-bearing settlement.
The user's final Merkle claim is an ordinary Ethereum transaction.

## Restarts and reorgs

On restart, in-progress jobs return to `queued` or `approved` according to
whether approval was already recorded. A persisted network request ID is
resumed rather than recreated.

After submission, the indexer waits for `ETHEREUM_CONFIRMATIONS`. If the
transaction is reorged, the gateway restores virtual Mina snapshots and
requeues the existing proof. Operators should stop new state writers until the
canonical chain and job status are understood.

## Safe test modes

| Mode | Behavior | Allowed environment |
| --- | --- | --- |
| `API_EXECUTE_ONLY=true` | Executes and validates SP1, then stops at `executed`; no proof and no Ethereum write. | Development and pre-deployment validation. |
| `API_LOCAL_MOCK_SUBMIT=true` | Executes SP1, then submits public values with empty proof bytes. | Chain ID 31337 with repository `LocalSP1Verifier` only. |
| Approval mode | Executes, pauses, then obtains a network proof after approval. | Persistent testnet. |

Execute-only and local-mock-submit are mutually exclusive. The gateway refuses
mock mode on Sepolia.
