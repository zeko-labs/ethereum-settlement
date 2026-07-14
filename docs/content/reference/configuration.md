# Configuration

The persistent reference profile splits immutable images, public deployment
identity, runtime settings, and secrets. Do not collapse them into one checked-in
environment file.

## Gateway runtime

| Variable | Purpose |
| --- | --- |
| `DATABASE_URL` | Gateway PostgreSQL connection. |
| `PROOF_API_KEY` | GraphQL mutation token and operator REST API key. |
| `API_BIND` | Listen address; use loopback/private networking. |
| `API_EXECUTE_ONLY` | Execute SP1 and stop without proving or submission. |
| `API_LOCAL_MOCK_SUBMIT` | Chain-31337-only empty-proof submission mode. |
| `API_REQUIRE_PROOF_APPROVAL` | Pause every paid job after preflight. Must be true on testnet. |
| `RPC_URL` | Sepolia JSON-RPC endpoint. |
| `SETTLEMENT_CONTRACT_ADDRESS` | Settlement proxy address. |
| `BRIDGE_CONTRACT_ADDRESS` | Bridge proxy address. |
| `SETTLEMENT_PRIVATE_KEY` | Settlement submitter key. |
| `BRIDGE_PRIVATE_KEY` | Bridge submitter key. |
| `WITHDRAW_PRIVATE_KEY` | Legacy withdraw submitter key; current preflight expects the same EOA as the other two. |
| `NETWORK_PRIVATE_KEY` | Succinct requester key. |
| `PROOF_SYSTEM` | `groth16` for EVM testnet submission. |
| `PROVER_TIMEOUT_SECS` | Maximum network proof wait; reference value 21600. |
| `PROVER_MIN_AUCTION_PERIOD_SECS` | Minimum auction period requested from the network. |
| `PROVER_MIN_REMAINING_SLOTS` | Minimum settlement lifetime required at approval. |
| `PROVER_GAS_LIMIT` | Deployment-wide maximum PGU. Required in approval mode. |
| `PROVER_MAX_PRICE_PER_PGU` | Deployment-wide maximum price. Required in approval mode. |
| `ETHEREUM_CONFIRMATIONS` | Canonical confirmation depth; reference profile requires 12. |
| `ETHEREUM_POLL_INTERVAL_SECS` | Receipt/indexer poll interval. |
| `ETHEREUM_INDEXER_START_BLOCK` | Contract deployment block. |
| `BRIDGE_AUTO_PROVE_DEPOSITS` | Queue each complete finalized native-deposit batch automatically. Enable for the browser PoC. |
| `BRIDGE_AUTO_PROVE_POLL_SECS` | Automatic batch scan interval; reference value 5. |
| `API_CORS_ALLOWED_ORIGINS` | Comma-separated browser origins, or `*` for isolated local development. |

`ETHEREUM_PRIVATE_KEY` is a development fallback for the three per-kind keys.
Use credential files and the explicit variables in a persistent deployment.

## Virtual Mina view

| Variable | Purpose |
| --- | --- |
| `VIRTUAL_MINA_GENESIS_TIMESTAMP` | Must match settlement's virtual-slot genesis. |
| `VIRTUAL_MINA_FORK_SLOT` | Must match settlement `forkSlot`. |
| `VIRTUAL_MINA_ACCOUNT_CREATION_FEE` | Mina-shaped response value. |
| `VIRTUAL_MINA_INITIAL_STATE_HASH` | Initial best-chain fallback before indexing. |
| `VIRTUAL_MINA_ACCOUNTS_PATH` | Read-only complete account array. |
| `VIRTUAL_MINA_OUTER_PUBLIC_KEY` | Outer account that receives indexed proof-emitted actions. |

The accounts file must contain the outer account and settlement fee payer with
state and nonce matching the deployed genesis. Existing database rows are not
overwritten at startup.

## Sequencer

The Ethereum-enabled sequencer needs:

```text
ZEKO_ETHEREUM_GATEWAY_TOKEN=<same logical secret as PROOF_API_KEY>
ZEKO_CIRCUITS_CONFIG=/config/circuits.json
ZEKO_SIGNATURE_KIND=zeko-testnet
```

and command-line values for gateway L1/archive URIs, three DA nodes/keys,
quorum two, `--inner-sync-period 30`, and the proof-bound commit validity
period.

## Immutable public files

The runtime config directory is mounted read-only:

| File | Source |
| --- | --- |
| `circuits.json` | Exact OCaml circuit config built with the final bridge proxy. |
| `bridge-genesis-ledger.json` | Genuine OCaml bridge export. |
| `bridge-scenario.json` | Public DA/sequencer/recipient identity and bridge checkpoint manifest. |
| `virtual-mina-accounts.json` | Outer and fee-payer GraphQL account objects. |
| `artifacts/manifest.json` | Chain, proxies, vkeys, VK identifier, DA mode, and holder address. |

Changing any of the first three after building the Zeko/gateway images creates
a different proof identity.

## Secret files

The Compose profile expects separate files for:

- proof API key and Succinct requester key
- Ethereum submission key files
- gateway/sequencer PostgreSQL passwords and RabbitMQ password
- sequencer private key and signer token
- three DA private keys and three signer tokens
- bridge-recipient private key
- signer TLS certificate and private key

Private files must be mode `0600` or `0400`; the public TLS certificate may be
`0644`. Prefer NixOS/systemd credentials or an external secret manager over
putting values in the Nix store or image layers.
