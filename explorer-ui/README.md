# Zeko Ethereum explorer

`explorer-ui/` is the standalone React application for Zeko L2 execution,
Ethereum settlement, and native bridge activity. It has no wallet or operator
credentials. All data comes from the gateway's public `/v1/explorer/*` API.

## Runtime configuration

The production server must provide `/runtime-config.json` without long-lived
caching:

```json
{
  "schemaVersion": 1,
  "gatewayUrl": "https://gateway.example",
  "bridgeUiUrl": "https://bridge.example",
  "ethereumExplorerUrl": "https://sepolia.etherscan.io",
  "networkName": "Zeko Testnet",
  "pollIntervalMs": 5000
}
```

Only `http` and `https` URLs are accepted. The browser must never receive
`PROOF_API_KEY`, PostgreSQL credentials, or Ethereum submitter keys. Configure
the gateway CORS allowlist for the explorer origin and route deep links back to
`index.html`.

## Development and verification

```sh
cd explorer-ui
pnpm install --frozen-lockfile
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
```

Development uses `127.0.0.1:5175`; preview uses `127.0.0.1:4175`. The UI polls
visible pages at the configured interval, stops polling while the document is
hidden, and re-fetches authoritative state when a tab becomes visible again.

All archive quantities and identifiers that may exceed JavaScript's safe
integer range remain decimal strings from API to rendering.
