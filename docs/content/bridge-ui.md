# Bridge web application

`bridge-ui/` is a standalone React application for the native ETH bridge PoC.
It is built and deployed independently from the Vue applications in
`~/zeko-ui`. The app composes the pinned Zeko bridge SDK with the Ethereum
wrapper SDK and uses only browser-safe APIs.

## User flows

The app supports the four user-owned boundaries of the round trip:

1. connect an injected Ethereum wallet and Auro
2. deposit native ETH to the bridge for a Zeko public key
3. prepare, sign, and submit deposit finalization through the sequencer
4. prepare, sign, and submit a native withdrawal request, then claim it on
   Ethereum when the public Merkle proof becomes claimable

Deposit batching, SP1 execution/proving, and Ethereum settlement submission
remain gateway/operator responsibilities. The app polls public deposit and
withdrawal endpoints; it never calls proof approval routes and never receives
`PROOF_API_KEY`.

## Auro signing domain

Auro currently assigns the built-in network ID `testnet` when a custom Mina
endpoint is added. For this PoC, the display name is configurable but the
signing ID is deliberately fixed:

```json
{
  "minaSigningNetworkId": "testnet",
  "auroNetworkName": "Zeko Ethereum PoC"
}
```

The same exact value must be used by Auro, the live Zeko circuit configuration,
the sequencer signer, and the deployment preflight. `Zeko Testnet` is a display
label only. A future Auro release that supports custom signing salts should be
handled as a versioned network migration rather than silently changing this
field.

## Runtime configuration

The static application loads `/runtime-config.json` with caching disabled.
Materialize it before deployment; it contains no secret values:

```sh
cd bridge-ui
BRIDGE_UI_GATEWAY_URL=https://gateway.example \
BRIDGE_UI_SEQUENCER_GRAPHQL_URL=https://sequencer.example/graphql \
BRIDGE_UI_ZEKO_ARCHIVE_GRAPHQL_URL=https://archive.example/graphql \
BRIDGE_UI_ACTIONS_API_URL=https://actions.example/graphql \
pnpm config:write
pnpm build
```

The generator deliberately does not expose a signing-network variable: it
always writes `testnet`. It accepts the remaining public limits, fee, explorer,
polling, and display-name settings through the `BRIDGE_UI_*` variables listed
in `bridge-ui/README.md`.

| Field | Meaning |
| --- | --- |
| `gatewayUrl` | Public gateway base URL for bridge discovery and status. |
| `sequencerGraphqlUrl` | Zeko sequencer GraphQL endpoint used for user submissions. |
| `zekoArchiveGraphqlUrl` | Mina-compatible archive endpoint used by the bridge SDK. |
| `actionsApiUrl` | Public Actions GraphQL endpoint used to prepare inclusion witnesses. |
| `expectedEthereumChainId` | `11155111` for this Sepolia-only PoC. |
| `minaSigningNetworkId` | Exact value `testnet`. |
| `auroNetworkName` | Wallet display name for the custom endpoint. |
| `zekoTransactionFeeNanomina` | Fee supplied to sequencer bridge operations, as a decimal string. |
| `ethereumExplorerUrl` / `zekoExplorerUrl` | Public transaction explorer bases. |
| `pollIntervalMs` | UI status polling period. |
| `maxDepositWei` | Client-side native deposit safety cap, as a decimal string. |

The gateway, sequencer, and Actions API must allow the deployed UI origin.
Terminate TLS and apply public rate limits at the reverse proxy; keep proof
operator and admin routes authenticated and inaccessible from the browser.

## Build and verify

```sh
cd bridge-ui
pnpm install --frozen-lockfile
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
```

For local development, `pnpm dev` listens on `127.0.0.1:5174`. The vendored SDK
archives are tied to the pinned `~/zeko-ui` source revision because the
published bridge SDK version does not expose the runtime entry points required
by the Ethereum wrapper. Update and test the pair together.
