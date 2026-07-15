# Ethereum ↔ Zeko bridge UI

Standalone React application for the native ETH bridge PoC. It talks directly
to the public gateway bridge API, gateway/sequencer GraphQL endpoints, injected
Ethereum wallets, and Auro. It never calls proof-operator routes.

## Run locally

```bash
pnpm install --frozen-lockfile
pnpm dev
```

The default URL is `http://127.0.0.1:5174`. Edit
`public/runtime-config.json` before deployment; it contains public endpoints and
limits only.

The checked-in file is ready for the local PoC. Operators can materialize the
same schema from environment variables before building:

```bash
BRIDGE_UI_GATEWAY_URL=https://bridge-gateway.example \
BRIDGE_UI_SEQUENCER_GRAPHQL_URL=https://sequencer.example/graphql \
BRIDGE_UI_ZEKO_ARCHIVE_GRAPHQL_URL=https://archive.example/graphql \
BRIDGE_UI_ACTIONS_API_URL=https://actions.example/graphql \
pnpm config:write
pnpm build
```

Pass a destination as the first argument to write directly into an existing
deployment, for example `pnpm config:write -- dist/runtime-config.json`. Use
`pnpm config:write -- --stdout` to inspect the generated JSON without writing.

Supported public variables:

| Variable | Default | Purpose |
| --- | --- | --- |
| `BRIDGE_UI_GATEWAY_URL` | `http://127.0.0.1:8080` | Public bridge gateway base URL |
| `BRIDGE_UI_SEQUENCER_GRAPHQL_URL` | `http://127.0.0.1:1923/graphql` | Zeko sequencer GraphQL and Auro custom-network URL |
| `BRIDGE_UI_ZEKO_ARCHIVE_GRAPHQL_URL` | sequencer URL on local PoC | Mina-compatible archive GraphQL URL |
| `BRIDGE_UI_ACTIONS_API_URL` | `http://127.0.0.1:9101/graphql` | Public actions preparation/index API |
| `BRIDGE_UI_ETHEREUM_CHAIN_ID` | `11155111` | Required Sepolia chain ID |
| `BRIDGE_UI_AURO_NETWORK_NAME` | `Zeko Ethereum PoC` | Auro custom-network display name |
| `BRIDGE_UI_ZEKO_FEE_NANOMINA` | `100000000` | Sequencer operation fee passed to the SDK |
| `BRIDGE_UI_ETHEREUM_EXPLORER_URL` | Sepolia Etherscan | Ethereum transaction links |
| `BRIDGE_UI_ZEKO_EXPLORER_URL` | Zeko testnet explorer | Zeko transaction links |
| `BRIDGE_UI_POLL_INTERVAL_MS` | `5000` | Visible-page gateway polling interval |
| `BRIDGE_UI_MAX_DEPOSIT_WEI` | `100000000000000000` | Browser-enforced experimental deposit cap |

`minaSigningNetworkId` is intentionally not configurable. The generator and
browser validator always require `testnet`, matching Auro's current signing salt
for custom networks.

Both Zeko bridge SDK packages are vendored from the same pinned source commit.
Do not replace only one with the same-numbered registry package: the published
bridge SDK artifact lacks runtime exports required by the Ethereum wrapper.

## Auro signing domain

Auro currently assigns the Mina network ID `testnet` to custom networks. This
PoC therefore adds the configured sequencer endpoint as `Zeko Ethereum PoC` and
constructs the bridge SDK runtime with both circuit networks set to `testnet`.
This is a temporary signing-domain compatibility choice, not the intended
production network ID.

## Validation

```bash
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
```

The production host must allow the UI origin through `API_CORS_ALLOWED_ORIGINS`
on both the gateway and sequencer-facing services.
