# Settlement

A settlement proves one genuine OCaml Zeko outer commit. The sequencer sends
the same zkApp command shape it used against Mina, plus an Ethereum settlement
export, to the gateway's Mina-compatible `sendZkapp` mutation.

## 1. OCaml produces the authoritative transition

The normal Zeko committer sequences the batch, writes it to 2-of-3 multisig
DA, obtains the Pickles proof from the OCaml prover, and exports:

- wrap verifier-index JSON and proof JSON
- complete Pickles public-input skeleton and two-field zkApp statement
- canonical account-update body random-oracle input
- the single eight-field outer `Commit` action
- all eight source outer-state fields
- the Mina tracking transaction hash and virtual account metadata
- for V2 through V4, the exact ordered inner actions and available native or
  ERC-20 withdrawal preimages
- for V3/V4, the proof-bound registry checkpoint and exact record identity or
  ordered record batch

The gateway adds only live Ethereum context: chain ID, settlement address, next
batch sequence, Mina tracking hash, and current outer action-state length. It
does not supply a trusted next Zeko state.

## 2. SP1 verifies and derives

The settlement guest is built against the exact exported verifier index. It:

1. verifies the Pickles accumulator and challenge-polynomial commitment
2. reconstructs and checks deferred values
3. reconstructs the wrap public input
4. verifies the outer Kimchi proof
5. recomputes the account-update body digest and compares it with the verified
   application statement
6. recomputes the action hash and decodes the proof-bound outer commit
7. derives the next eight-field outer state, action states and lengths, synchronized
   checkpoint, and slot range
8. for V2 through V4, replays the exact inner actions to the proof-bound state
   and creates the Keccak claim tree
9. for V3/V4, verifies the registry transition and derives its checkpoint and
   record attestation

Mutating the application statement, deferred values, bulletproof challenges,
accumulator point, feature flags, previous evaluations, body, or actions causes
verification to fail.

## 3. Versioned public values

The byte layouts are fixed and use big-endian integers.

| Receipt | Length | Contents |
| --- | ---: | --- |
| V1 (`ZKST`, version 1) | 768 bytes | Multisig DA mode, Ethereum domain, batch/VK/statement identifiers, state before/after, outer and synchronized action checkpoints, and slot bounds. |
| V2 (`ZKST`, version 2) | 828 bytes | The V1 fields plus bridge address, depth-16 inner-action root, global start index, and action count. |
| V3 (`ZKST`, version 3) | 932 bytes | The V2 fields plus registry root, count, schema version, record hash, and canonical Mina record commitment. |
| V4 (`ZKST`, version 4) | 904 bytes | The V2 fields plus registry root, count, schema version, ordered record-batch root, and record count. |

The eight outer-state fields are the OCaml `Rollup_state.Outer_state` layout:

| Index | Meaning |
| ---: | --- |
| 0 | Pause key |
| 1 | Status / paused-emergency flags |
| 2 | Ledger hash |
| 3 | Inner action state |
| 4 | Inner action-state length |
| 5 | Sequencer key |
| 6 | DA key or multisig identity |
| 7 | Account-set commitment |

The PoC `vkHash` is SHA-256 of the exact verifier-index JSON embedded in the
guest. This is an artifact-identity check, not the production canonical Mina
verification-key hash.

## 4. Ethereum acceptance

`ZekoSettlement.verifyAndUpdateRoot` first verifies the SP1 proof under the
configured program vkey. It then requires:

- multisig DA mode, current chain ID, and its own proxy address
- exactly the next batch sequence and configured Pickles VK identifier
- exact equality of all eight stored source-state fields
- exact outer action state and length continuity
- one new outer commit action
- a known synchronized outer checkpoint whose length does not exceed the
  available outer action length
- a valid slot interval containing the contract's virtual Mina slot

On success it stores the complete next outer state, records the new outer and
inner checkpoints, and emits `SettlementAccepted`.

For V2 through V4 it additionally checks the configured bridge address and that
the inner action count equals the proof-bound length delta. The root is stored
under the accepted settlement sequence and cannot be installed independently
from that settlement. V3 records one exact registry record hash and canonical
Mina Poseidon commitment. V4 records a depth-8 Keccak batch root over ordered
`(recordHash, recordCommitment)` leaves so pending Solidity proposals can be
activated only with the settled pair.

## 5. Gateway lifecycle

Every testnet settlement is first verified natively against the same baked
Pickles verifier used by the guest, and its receipt is produced by the shared
derivation code. In approval mode it pauses at `awaiting_approval`; only an
operator-approved digest and per-job cost caps may proceed to the Succinct
Network. With the required PGU cap, the network request skips redundant zkVM
simulation. The resulting Groth16 proof is submitted and held in `submitted`
until its receipt block reaches Ethereum consensus finality.

Confirmed state updates the gateway's virtual Mina account, action, pending
pool, and best-chain views. A reorg restores their prior snapshots and reuses
the existing proof request.

See [proof jobs and approval](/gateway/proving) for the paid boundary.
