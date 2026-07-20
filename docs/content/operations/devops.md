# DevOps and NixOS deployment delta

This page describes what must be deployed **in addition to the existing Mina
Zeko stack**. It is based on the current `~/machines` flake and the working
reference profile under `deploy/testnet`.

## Existing Mina Zeko baseline

The machines repository currently defines these testnet roles:

| Host group | Existing services |
| --- | --- |
| `testnet-sequencer` | Sequencer, local OCaml prover, RabbitMQ, PostgreSQL, one DA node/signer, Zeko archive/relay/API, nginx, scheduled test transaction. |
| `testnet-prover-1` … `testnet-prover-19` | Remote OCaml prover workers consuming the sequencer RabbitMQ. |
| `testnet-da-node-2` | A second managed DA node and signer. |
| `testnet-mina-daemon-1` | Mina devnet daemon, Mina archive, PostgreSQL, and archive API. |
| `observability` | OpenTelemetry ingestion, VictoriaMetrics/Logs/Traces, Loki, Grafana, and Alertmanager. |

The sequencer also lists external DA nodes. In the current Nix module,
`--da-quorum` is hardcoded to `1`, the testnet circuit config has a 1-of-1
multisig key, the L1 URI comes from the Mina daemon setting, and the archive URI
is a Mina archive endpoint.

The flake pins Zeko `v1.0.5`. It has no gateway, Ethereum contract, SP1,
Succinct, or Ethereum RPC service definitions. Those facts make the current
machines flake a baseline, not an Ethereum-PoC deployment.

## Required Ethereum PoC additions

| Addition | Deployment requirement |
| --- | --- |
| Gateway host/service | Run the pinned `zeko-proof-api` image or Nix package with a dedicated system user. It performs CPU-heavy local SP1 execution, so a separate x86_64 host is preferred over sharing the sequencer. |
| Gateway PostgreSQL | Persistent private database for jobs, virtual Mina accounts/actions, Ethereum blocks, bridge logs, proof metrics, and rollback snapshots. Protocol state is replayable, but retain backups for proof-cost/audit history. |
| Ethereum RPC | Reliable Sepolia JSON-RPC with archive/log access from the deployment block. Use a redundant provider pair or operate synchronized execution and consensus clients. |
| Ethereum contracts | Real SP1 verifier plus deterministic settlement and bridge implementation/proxy deployments. Contract deployment is a release step, not a long-running Nix service. |
| Succinct requester | Network private key, funded PROVE balance, Groth16 configuration, hard PGU/price caps, and six-hour worker timeout. |
| Ethereum transaction signer | Funded Sepolia EOA holding only `PROVER_ROLE`. Current preflight expects the settlement, bridge, and legacy-withdraw submitter files to resolve to this same address. |
| Retained PoC identity | Final bridge address, circuit config, genesis ledger, three DA keys, sequencer/recipient keys, verifier index, SP1 vkeys, and manifest stored as one release unit. |
| Three managed DA nodes | For this PoC, deploy exactly three retained DA identities and configure quorum two. Existing external/quorum-one testnet settings are not the target identity. |
| Reverse proxy policy | Private/authenticated access for `/graphql` and proof operator routes; optionally public, rate-limited bridge discovery routes. |
| Monitoring and backup | Gateway/systemd health, job-state age, balances, slot lifetime, finalized-head lag, DA quorum, database backup, and immutable artifact retention. |

No blob sidecar, beacon blob fetcher, blob archive, or blob transaction service
is needed for this milestone.

## Required sequencer changes

The Ethereum PoC must run the companion Zeko commit containing the settlement
export changes; the current `v1.0.5` flake input is insufficient unless that
code has been released under a new immutable ref.

Change the NixOS sequencer module so these are options rather than hardcoded
Mina values:

| Setting | Ethereum PoC value |
| --- | --- |
| `--l1-uri` | Private gateway `http(s)://.../graphql` |
| `--archive-uri` | The same gateway GraphQL endpoint |
| `ZEKO_ETHEREUM_GATEWAY_TOKEN` | Credential file shared only with sequencer and gateway |
| `--inner-sync-period` | `30` seconds in the reference profile |
| `--da-node` | The three retained PoC DA endpoints |
| `--da-keys` | Exactly the three retained public keys |
| `--da-quorum` | `2` |
| `--commitment-period` | `${ZEKO_COMMITMENT_PERIOD_SECONDS:-900}` in the Ethereum reference profile; Mina defaults are unchanged |
| `--slot-duration` | `${ZEKO_SLOT_DURATION_SECONDS:-12}` seconds; must match `ZekoSettlement.slotDuration` |
| `--commit-validity-period` | `2400` slots |
| circuit config | Exact file built with the final bridge proxy address |

Keep the OCaml prover and RabbitMQ path. The gateway performs SP1 proving, not
Pickles proving, so it does not replace the existing prover fleet.

Give the gateway private network access to the sequencer GraphQL endpoint and
set `SEQUENCER_GRAPHQL_URL`. This is a read-only observability dependency: a
failure removes the explorer countdown but must not affect proof processing or
sequencer operation.

The Mina daemon and Mina archive may remain online for the existing Mina Zeko
network, but the Ethereum-PoC sequencer must not use them as its settlement L1
or outer-action archive.

## Recommended NixOS modules

Add a separate network/host profile rather than mutating the existing live
testnet in place. A practical machines-repository change is:

```text
hosts/ethereum-poc-gateway/vars.nix
configs/ethereum-poc.json
modules/ethereum-gateway.nix
modules/ethereum-poc-sequencer.nix   # or options in sequencer.nix
modules/ethereum-poc-monitoring.nix
```

`ethereum-gateway.nix` should define:

- a `zeko-gateway` user and state directory
- PostgreSQL database/user with local or private-socket authentication
- the gateway systemd unit with `Restart=on-failure`
- credential-backed environment using `LoadCredential=` or equivalent,
  never secrets in the Nix store
- read-only circuit/accounts/manifest files
- startup ordering on PostgreSQL and an RPC health precheck
- `ProtectSystem=strict`, `ProtectHome=true`, `PrivateTmp=true`,
  `NoNewPrivileges=true`, restricted address families, and a writable state
  directory
- loopback/private bind on port 8080 and nginx TLS/rate-limit rules
- database backup and restore procedure tested before launch
- immutable deployment block, virtual-account genesis, and outer/inner/fee-payer public keys required by chain replay

Package the gateway from a fixed repository revision and fixture VK. Do not run
`cargo build` or fetch the verifier index on the production host at service
startup.

## Network layout

Only the reverse proxy and, where intended, the sequencer GraphQL endpoint are
edge-facing.

| Port/service | Exposure |
| --- | --- |
| Gateway `8080` | Loopback or private network; proxy selected routes. |
| Sequencer `1923` | Existing public policy through nginx, or private for the PoC. |
| PostgreSQL `5432` | Local/private only. |
| RabbitMQ `5672`, management `15672` | Prover network/private only; management should not be public. |
| DA RPC/health | Sequencer/bootstrap network only. |
| Signers `9000`–`9003` | Service network only with TLS and distinct bearer tokens. |
| Ethereum RPC | Egress to provider, or private access to self-hosted clients. |

The Compose reference uses an internal backend network and binds gateway and
sequencer to `127.0.0.1` by default. Preserve that boundary in NixOS firewall
rules.

## Secrets and roles

Provision, rotate, and back up these classes separately:

- Mina sequencer key and signer token/TLS material
- three Mina DA keys and three distinct signer tokens
- bridge demo recipient key
- gateway API key
- Succinct network requester key
- Ethereum prover/submission key
- PostgreSQL and RabbitMQ credentials
- contract default-admin, admin, and upgrader keys outside the runtime host

The existing machines README already describes signer TLS files under
`/var/lib/sequencer` and `/var/lib/da-layer`. Reuse that pattern, but issue a
certificate whose SAN matches each configured signer hostname. Never copy
fixture-only private keys into a gateway image.

## Observability additions

Extend the existing process/systemd exporter regexes and vmalert rules to cover
the gateway and its PostgreSQL instance. At minimum alert on:

- gateway service failure or restart loop
- `/health` failure or Ethereum chain-ID mismatch
- a job stuck in `validating`, `awaiting_approval`, `proving`, `submitted`, or
  `reorged` beyond its expected window
- remaining settlement slots approaching `PROVER_MIN_REMAINING_SLOTS`
- Succinct requester or Ethereum submitter balance below reserve
- proof quote/cost above policy, network proof failure, or contract revert
- finalized-head lag/reorg and indexer head lag
- bridge deposit nonce divergence or native liability exceeding contract balance
- fewer than two healthy retained DA nodes
- PostgreSQL disk growth, backup failure, and RabbitMQ consumer loss

Do not log private proof-network keys, Ethereum keys, gateway tokens, or full
secret environment dumps. Proof input and public values are not secrets, but
they can be large; store them in PostgreSQL/artifact storage rather than
unbounded journal fields.

## Deployment order

1. Create a new Ethereum-PoC machines/network profile and retained identities.
2. Predict the bridge proxy and bake it into the circuit config.
3. Generate and validate the real OCaml two-commit fixture.
4. Build immutable Zeko/gateway images and record digests plus SP1 vkeys.
5. Deploy contracts, configure roles/delay, and archive deployment receipts.
6. Materialize virtual accounts and gateway config from the same fixture.
7. Deploy PostgreSQL and gateway; verify health and vkey checks.
8. Deploy three DA nodes/signers, bootstrap the exact ledger, and verify 2-of-3.
9. Start RabbitMQ/prover; wait for the consumer readiness barrier.
10. Start the sequencer pointed at gateway GraphQL.
11. Run execute-only acceptance, then the approval-gated Sepolia round trip.

Rollback means stopping new sequencer commits and bridge proof requests,
preserving databases and manifests, and diagnosing the canonical state. Do not
redeploy a proxy or change the circuit config to “fix” an identity mismatch.

For DA-node replacement, start an empty node with
`--restore-from-peer HOST:PORT`. If the peer retained competing tips, select
one explicitly with `--restore-target-ledger-hash HASH`. This is a trusted-peer
copy with chain-continuity checks, not an independent ledger-root
recomputation. Follow the complete [recovery and rebuild](/operations/recovery)
drill before testnet launch.
