# Configuration

The persistent reference profile splits immutable images, public deployment
identity, runtime settings, and secrets. Do not collapse them into one checked-in
environment file.

## Gateway runtime

| Variable | Purpose |
| --- | --- |
| `DATABASE_URL` | Gateway PostgreSQL connection. |
| `ARCHIVE_DATABASE_URL` | Optional read-only Zeko archive PostgreSQL connection used only by public explorer reads. |
| `SEQUENCER_GRAPHQL_URL` | Optional internal sequencer GraphQL endpoint. When set, `/v1/explorer/summary` includes the exact commit-loop phase and schedule. |
| `PROOF_API_KEY` | GraphQL mutation token and operator REST API key. |
| `API_BIND` | Listen address; use loopback/private networking. |
| `API_EXECUTE_ONLY` | Execute SP1 and stop without proving or submission. |
| `API_LOCAL_MOCK_SUBMIT` | Empty-proof submission mode. Restricted to chain 31337 unless the explicit Sepolia PoC override is enabled. |
| `API_UNSAFE_ALLOW_MOCK_ON_SEPOLIA` | Permit `API_LOCAL_MOCK_SUBMIT` on Sepolia when every contract is wired to `LocalSP1Verifier`. Insecure PoC use only. |
| `API_REQUIRE_PROOF_APPROVAL` | Pause every paid job after preflight. Must be true on proof-verified testnet and false in mock mode. |
| `RPC_URL` | Sepolia JSON-RPC endpoint. |
| `SETTLEMENT_CONTRACT_ADDRESS` | Settlement proxy address. |
| `BRIDGE_CONTRACT_ADDRESS` | Bridge proxy address. |
| `SETTLEMENT_PRIVATE_KEY` | Settlement submitter key. |
| `BRIDGE_PRIVATE_KEY` | Bridge submitter key. |
| `NETWORK_PRIVATE_KEY` | Succinct requester key. |
| `PROOF_SYSTEM` | `groth16` for EVM testnet submission. |
| `PROVER_TIMEOUT_SECS` | Maximum network proof wait; reference value 21600. |
| `PROVER_MIN_AUCTION_PERIOD_SECS` | Minimum auction period requested from the network. |
| `PROVER_MIN_REMAINING_SLOTS` | Minimum settlement lifetime required at approval. |
| `PROVER_GAS_LIMIT` | Deployment-wide maximum PGU. Required in approval mode. |
| `PROVER_MAX_PRICE_PER_PGU` | Deployment-wide maximum price. Required in approval mode. |
| `ETHEREUM_FINALITY_MODE` | `finalized` reads Ethereum's consensus-finalized JSON-RPC tag and is required on testnet. `confirmations` is restricted to chain ID 31337. |
| `ETHEREUM_CONFIRMATIONS` | Depth used only when `ETHEREUM_FINALITY_MODE=confirmations`; local E2E uses 1. |
| `ETHEREUM_POLL_INTERVAL_SECS` | Receipt/indexer poll interval. |
| `ETHEREUM_INDEXER_START_BLOCK` | Contract deployment block. |
| `BRIDGE_AUTO_PROVE_DEPOSITS` | Queue each complete finalized deposit batch automatically. Enable for the browser PoC. |
| `BRIDGE_AUTO_PROVE_POLL_SECS` | Automatic batch scan interval; reference value 5. |
| `API_CORS_ALLOWED_ORIGINS` | Comma-separated browser origins, or `*` for isolated local development. |

`ETHEREUM_PRIVATE_KEY` is a development fallback for both per-kind keys.
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
| `VIRTUAL_MINA_INNER_PUBLIC_KEY` | Inner account whose canonical archive actions supply pending withdrawals and recoverable leaf preimages. |
| `VIRTUAL_MINA_FEE_PAYER_PUBLIC_KEY` | Settlement fee payer used when rebuilding the virtual Mina view from accepted Ethereum calldata. |
| `WITHDRAWAL_RECOVERY_POLL_SECS` | Archive/root reconstruction interval; reference value 5. |

The Ethereum-backed Sepolia fee schedule preserves the relative values of the
former Mina fees. It was calibrated on 2026-08-19 from approximately
$0.0407/MINA and $1,625/ETH, then normalized to 25,000 native nanounits per
former MINA. Consequently, `VIRTUAL_MINA_ACCOUNT_CREATION_FEE` is `25000`, the
L2 account-creation and ordinary operation fees are `2500`, the minimum
transaction fee is `250`, and the circuit-derived bridge-proof fee is `5000`.
Every value is a multiple of 10 native nanounits.

The accounts file must contain the outer account and settlement fee payer with
state and nonce matching the deployed genesis. The configured account-creation
fee is synchronized at startup; existing account rows are not overwritten.

## Sequencer

The Ethereum-enabled sequencer needs:

```text
ZEKO_ETHEREUM_GATEWAY_TOKEN=<same logical secret as PROOF_API_KEY>
ZEKO_CIRCUITS_CONFIG=/config/circuits.json
ZEKO_SIGNATURE_KIND=testnet
```

and command-line values for gateway L1/archive URIs, three DA nodes/keys,
quorum two, `--inner-sync-period 30`, and the proof-bound commit validity
period. Set `--slot-duration` to the settlement contract's virtual slot
duration; the Sepolia reference profile reads `ZEKO_SLOT_DURATION_SECONDS` and
uses 12 seconds. Mina deployments retain the 180-second CLI default.

The Ethereum reference profile sets `--commitment-period` from
`ZEKO_COMMITMENT_PERIOD_SECONDS`, defaulting to 900 seconds. This changes only
the Ethereum Compose profile; the OCaml CLI and Mina deployment defaults stay
unchanged. The sequencer exposes this live schedule through `commitSchedule`.

The Ethereum profile runs the sequencer with `--deposit-delay-blocks 0` because
the gateway only exposes consensus-finalized outer actions. Mina deployments
keep the sequencer's existing block-delay behavior; no OCaml finality logic is
changed by the Ethereum adapter.

An ERC-20-enabled sequencer supplies one universal registry configuration:

```text
--ethereum-bridge-address 0x...
--ethereum-asset-registry-l2 B62...
--ethereum-registration-authority-l2 B62...
--ethereum-shared-vault-l2 B62...
--ethereum-mft-standard-vk-id 9001
--ethereum-mft-token-vk-hash 0x...
--ethereum-mft-admin-vk-hash 0x...
--ethereum-universal-bridge-vk-id 9002
--ethereum-universal-bridge-vk-hash 0x...
```

The two MFT hashes identify the exact standard token and admin verification
keys. The universal bridge hash is derived from the provisional circuit
configuration before that configuration is finalized. The registry schema and
depth are circuit constants. Schema V1 uses a depth-8 tree with a 256-record
capacity. Individual asset records supply the Ethereum token, asset ID, dynamic
MFT owner, circuit-derived token ID, at most nine decimals, and inventory cap
through authenticated registry membership. Startup rejects a partial universal
configuration. Registration rejects an owner equal to the shared vault and any
record whose MFT or universal VK identifier differs from this configuration.

For this PoC, `MINA_SIGNING_NETWORK_ID=testnet` is the source value used to
materialize `ZEKO_SIGNATURE_KIND`. Auro assigns that built-in signing domain to
custom endpoints, and the MetaMask Mina Snap applies the same mapping for Zeko
testnet. Do not substitute the display name or `zeko-testnet`; signatures and
circuit commitments must use the same salt.

## Browser application

The standalone `bridge-ui/` build reads public configuration from
`/runtime-config.json`. It contains the gateway, sequencer/archive, and Actions
URLs, Ethereum chain ID, display names, fee, and polling interval.
It must contain `minaSigningNetworkId: "testnet"` and must never contain the
gateway proof API key, Ethereum submitter key, or Succinct requester key. See
[bridge web application](/bridge-ui) for the schema and deployment boundary.

The standalone `explorer-ui/` build also reads `/runtime-config.json`. It
contains only the public gateway base, bridge UI link, Sepolia explorer base,
network display name, and polling interval. See the
[L2 and settlement explorer](/explorer). The gateway's archive credentials stay
server-side and must belong to a transaction-read-only role.

## Immutable public files

The runtime config directory is mounted read-only:

| File | Source |
| --- | --- |
| `circuits.json` | Exact OCaml circuit config built with the final bridge proxy. |
| `bridge-genesis-ledger.json` | Genuine OCaml bridge export. |
| `bridge-scenario.json` | Public DA/sequencer/recipient identity and bridge checkpoint manifest. |
| `virtual-mina-accounts.json` | Outer and fee-payer GraphQL account objects. |
| `artifacts/manifest.json` | Chain, proxy/implementation/registry-module addresses, registry identity, vkeys, VK identifier, DA mode, and holder address. |

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

`tools/init-testnet-secrets.sh` creates the self-signed development signer
certificate through `tools/generate-local-signer-certificate.sh`. Its default
lifetime is 30 days; set `ZEKO_SIGNER_CERT_DAYS` to another positive number of
days when generating a retained environment. Restarting a retained stack does
not extend an existing certificate, so rotate it before `notAfter` and restart
the signer clients and servers together.
