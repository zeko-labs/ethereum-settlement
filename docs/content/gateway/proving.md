# Proof jobs and approval

The gateway owns all SP1 work. The sequencer and bridge users never hold a
Succinct requester key and never generate a proof themselves.

## Job lifecycle

In the testnet profile, a new job follows:

```text
queued -> validating -> awaiting_approval -> approved
  -> proving / proof_requested -> submitting -> submitted -> confirmed
```

`validating` verifies every proof input before any paid request exists. A
settlement is checked natively with the same baked Pickles verifier blob used
by the SP1 guest, then its receipt is derived by the exact shared guest
function. This avoids a roughly 5-billion-cycle zkVM replay on the current
optimized guest; the retained July 15 audit checkpoint predates that
optimization and records roughly 52 billion cycles. Bridge and legacy-withdraw
jobs still execute their SP1 guests through the low-memory executor. The
gateway validates the resulting public values against live contract state and
hashes the hydrated proof input.

Native settlement validation deliberately records `cycleCount: null`; native
runtime is not an SP1 cycle or network-PGU measurement. Set
`API_EXECUTE_ONLY=true` on a separate audit gateway to force the complete
settlement guest through the zkVM when guest/host equivalence needs to be
rechecked.

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

Executor cycles are not network PGU. Native settlement jobs have no cycle
count at all. Supply an explicit PGU cap from a reviewed SP1 simulation or a
deliberately conservative operator budget. The quote reads current
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
Approval mode always supplies the configured `PROVER_GAS_LIMIT`; the SP1
Network request therefore skips its redundant zkVM simulation and uses the
reviewed cap directly.

## Cost accounting

When available, each job records:

- local execution cycles when the zkVM executor was used
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

After submission, the indexer waits until the receipt block is at or below the
JSON-RPC `finalized` head. Only then does it apply proof-emitted actions to the
virtual Mina view. If the transaction is reorged before finality, the job stays
submitted. A conflict with an already-finalized block is an RPC/consensus
invariant violation and the indexer fails closed.

`ETHEREUM_FINALITY_MODE=confirmations` retains the old depth check solely for
local chain ID 31337, including Anvil whose `finalized` tag does not advance.
The gateway rejects it on other chain IDs, and testnet preflight rejects it.

## Safe test modes

| Mode | Behavior | Allowed environment |
| --- | --- | --- |
| `API_EXECUTE_ONLY=true` | Forces every guest, including settlement, through the zkVM and stops at `executed`; no proof and no Ethereum write. | Development and pre-deployment audit. |
| `API_LOCAL_MOCK_SUBMIT=true` | Uses native settlement verification and zkVM bridge/withdraw validation, then submits public values with empty proof bytes. | Chain ID 31337 with repository `LocalSP1Verifier` only. |
| Approval mode | Uses the operational preflight, pauses, then obtains a network proof with the approved explicit PGU cap. | Persistent testnet. |

Execute-only and local-mock-submit are mutually exclusive. The gateway refuses
mock mode on Sepolia.
