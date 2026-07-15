# L2 and settlement explorer

`explorer-ui/` is a standalone React explorer for the Ethereum-settled Zeko
testnet. It brings three independently authoritative views into one public
interface:

- Zeko L2 blocks, user commands, zkApp commands, account updates, and observed
  account state from the OCaml archive PostgreSQL schema
- SP1 settlement job progress and canonical `SettlementAccepted` /
  `InnerActionBatchAccepted` events indexed from Ethereum
- native deposits, synchronized outer actions, withdrawal inclusion proofs,
  delay state, and canonical claim events from the gateway
- exact periodic-commit phase and timestamps fetched server-side from the
  sequencer GraphQL API

The browser never connects to PostgreSQL, never receives an API key, and never
calls proof quote, approval, cancellation, or submission routes.

## Record model

The current sequencer produces one user or zkApp transaction per block. Block
and transaction pages preserve the archive's canonical/pending status, command
hash, fee payer, memo, nonce, and zkApp account-update order. Account pages show
the latest canonical `accounts_accessed` state plus recent referencing
transactions.

Settlement pages intentionally call the OCaml value a **settlement command
digest** rather than an L2 transaction hash. Public responses include accepted
state/action checkpoints, slot range, inner action root, confirmation count,
gas, and cycle count. They omit proof inputs, approval digests, proof request
IDs, operator errors, and costs.

Deposit status comes from finalized Ethereum logs and gateway synchronization.
The UI does not claim a canonical deposit-nonce-to-L2-finalization mapping,
because the archive schema does not persist one. Withdrawal pages use the
gateway's authoritative Merkle proof and live virtual-slot/cursor state, then
link canonical claim transactions when indexed.

Every amount, height, nonce, sequence, and slot that can exceed JavaScript's
safe integer range is returned and rendered as a decimal string.

The overview shows a live **Next commit** metric. While the sequencer is
waiting it counts down to `nextAttemptAt`; while a commit is being assembled it
shows elapsed time from `lastAttemptStartedAt`. The browser advances this clock
once per second using the summary `asOf` value, without increasing the normal
gateway polling rate. If the internal sequencer endpoint is unavailable, only
this metric becomes unavailable.

## Public endpoints

All endpoints are `GET` routes under `/v1/explorer`:

| Route | Data |
| --- | --- |
| `/summary` | Source health, aggregate L2/settlement/bridge metrics, and optional sequencer commit schedule. |
| `/search?q=…` | Exact block, transaction, account, settlement, deposit, and withdrawal matches. |
| `/blocks`, `/blocks/:height-or-hash` | Zeko archive blocks and their single transaction. |
| `/transactions`, `/transactions/:hash` | User/zkApp commands and ordered account updates. |
| `/accounts/:publicKey` | Latest canonical account state and recent commands. |
| `/settlements`, `/settlements/:id-or-sequence` | Public proof progress merged with canonical Ethereum events. |
| `/deposits`, `/deposits/:nonce` | Native deposit progress and synchronization. |
| `/withdrawals`, `/withdrawals/:sequence/:offset` | Inclusion, delay, cursor, and claim state. |

List routes accept an opaque `cursor` and a bounded `limit` of 1–100. Relevant
routes also accept `status`, `kind`, or `account` filters. Cursors are opaque;
clients must not parse them.

## Archive connection

Set `ARCHIVE_DATABASE_URL` on the gateway to a dedicated read-only archive
role. The persistent Compose profile creates `zeko_explorer`, enforces
read-only transactions and a five-second statement timeout, and grants only
schema usage and table selection. The archive database stays on the internal
network.

During a first bootstrap the gateway precedes sequencer schema creation. L2
routes return a temporary source-unavailable response until the archive tables
exist; browser polling then recovers automatically. Gateway-backed settlement
and bridge indexing continues independently.

## Build and deploy

```sh
cd explorer-ui
pnpm install --frozen-lockfile
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
```

Serve `dist/` as a history-fallback static application and materialize the
public `runtime-config.json` documented in `explorer-ui/README.md`. Apply exact
CORS origins and route-specific rate limits at the TLS reverse proxy.
