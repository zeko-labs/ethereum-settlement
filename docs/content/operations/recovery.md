# Recovery and rebuild

The PoC is recoverable when at least one copy of every recovery root survives.
Deleting a service database should not make that database the protocol source
of truth, but deleting all roots at once is not recoverable.

## Recovery roots

| Root | Must survive | What it reconstructs |
| --- | --- | --- |
| Ethereum execution/consensus history from `ETHEREUM_INDEXER_START_BLOCK` | Contract state, receipts, logs, and accepted transaction calldata | Deposits, bridge transitions, settlements, claims, virtual outer actions/account state, and accepted inner roots. |
| Sequencer DA-diff store or tested DA snapshot | Ordered Zeko ledger diffs beyond genesis | Replacement DA nodes. |
| Canonical Zeko archive rebuilt from DA/sequencer data | Inner action fields and clear withdrawal preimages | Pending withdrawal activity and settled Merkle leaves. |
| Immutable release identity | Contract addresses/deployment block, circuit config, bridge genesis ledger, virtual Mina genesis accounts, outer/inner/fee-payer keys, DA keys, VK/vkeys | Correct interpretation of all replayed data. |

On local Anvil, the Anvil state itself is the Ethereum recovery root. Removing
it removes the local chain and deployed contracts; no service can recover that
chain from the gateway database.

## Service matrix

| Service erased | Bootstrap source | Remaining limitation |
| --- | --- | --- |
| Gateway PostgreSQL | Ethereum events/calldata plus immutable virtual accounts; archive preimages for withdrawals | Succinct quotes, approval decisions, request IDs, and historical operator errors are operational audit data and require a database backup/export. |
| Gateway process | Its PostgreSQL database, then normal Ethereum/archive reconciliation | No limitation beyond dependencies. |
| Actions indexer/API | Canonical archive through gateway `/archive/graphql`, plus gateway outer-action GraphQL | Reindex time; API itself is stateless. |
| Explorer UI / bridge UI | Static build plus runtime config and public APIs | Wallet authorization may still require the wallet extension to remain authorized. |
| Archive | Sequencer/DA replay using the normal Zeko archive pipeline | Must preserve ordered blocks/commands and action preimages. |
| Sequencer | Genesis/release identity plus surviving DA chain | In-flight mempool commands that never reached DA are not protocol-recoverable. |
| One DA node | Sequencer replay through the normal ordered post-diff flow, or a tested node-volume snapshot | DA nodes do not synchronize from peers; replay must complete before the node counts toward quorum. |
| All DA nodes | Sequencer DA-diff store or tested snapshots | Ethereum checkpoints contain hashes, not the full L2 ledger/diffs. Without the sequencer's ordered data or snapshots, recovery is impossible. |

## Gateway replay

On an empty database the gateway:

1. loads immutable virtual Mina genesis accounts
2. indexes canonical Ethereum blocks from the deployment block
3. indexes deposits, `BridgeTransitionAccepted`, `SettlementAccepted`, inner-root, and claim events
4. waits for configured Ethereum finality
5. decodes accepted bridge/settlement public values from transaction calldata
6. replays bridge and settlement actions in Ethereum block/log order
7. reconstructs settled inner leaves from canonical archive actions and writes them only when the complete root equals the accepted Ethereum root

The virtual fee-payer nonce starts in the immutable genesis account and
increments once per accepted settlement. New GraphQL settlement submissions
must equal that current nonce, so failed proof jobs cannot introduce replay-
invisible gaps.

Recovered proof-job rows exist only to preserve the existing account-history
and reorg machinery. They are marked `recoveredFromEthereum`; they do not imply
a new SP1 request or proof. A reorg removes those synthetic rows and replay
applies the new canonical events.

On every gateway process start, settlement mutation handling and the proof
worker remain recovery-gated until the first complete canonical replay tick.
Read-only bridge/explorer endpoints remain available while indexing. This
prevents the sequencer from creating a commit against partially replayed
virtual state and prevents a paid proof request from starting early.

## Destructive drill

Before testnet launch, retain chain/DA/archive data, genesis files, and secrets,
then perform the following in a disposable environment:

1. record contract cursors, virtual outer account/action state, deposit states,
   settlement sequences, pending withdrawals, and claimable leaf proofs
2. stop the gateway and delete only its PostgreSQL volume
3. start a fresh database/gateway with the same immutable config and deployment block
4. verify the recorded protocol state and leaf roots reappear without submitting or proving anything
5. delete one DA-node volume, repopulate it from the sequencer's ordered diffs, and compare its reported diff chain
6. rebuild Actions services from the archive and compare action indices/account-update IDs
7. test a service restart and an Ethereum reorg separately; do not combine root deletion with the reorg test

Back up gateway PostgreSQL even though protocol state replays: paid proof
quotes, approvals, Succinct request/cost evidence, and operational diagnostics
are required for auditability but are not encoded completely on Ethereum.
