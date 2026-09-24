# Bridge web application

The [bridge UI README](https://github.com/zeko-labs/ethereum-settlement/blob/main/bridge-ui/README.md) owns supported assets, user
flows, registry discovery, and activity recovery. The application is built and
deployed independently from the Vue applications in the companion Zeko UI
repository and uses only browser-safe APIs.

## Connect your wallets

The bridge connects two accounts with different roles:

- **Ethereum wallet:** a `0x…` account for Ethereum deposits, ERC-20 approvals,
  and withdrawal claims. Use the Ethereum network selected by the deployment
  (Sepolia for the testnet).
- **Zeko wallet:** a `B62…` account for receiving bridged assets and signing
  Zeko transactions. Choose Zeko Wallet (the MetaMask Snap) or Auro. The Snap
  can use the same MetaMask installation as your Ethereum wallet, but the
  accounts and transaction approvals remain separate.

The bridge configures the Zeko endpoint after connection. MetaMask Snap users
must approve the requested network endpoint and account access. For local Snap
builds, follow the [MetaMask Flask setup](https://github.com/zeko-labs/ethereum-settlement/blob/main/mina-snap/README.md#try-it-locally).

## Bridge ETH and tokens

1. Connect both wallets and select ETH or a supported registered ERC-20.
2. Deposit on Ethereum to your Zeko account, approving ERC-20 spending if
   requested. Wait for Ethereum finality, deposit processing, and the Zeko
   settlement that synchronizes the deposit; then complete the Zeko-side
   finalization when available.
3. To return funds, request a withdrawal from Zeko to your Ethereum address.
   After settlement inclusion and the withdrawal delay, claim the funds with
   your Ethereum wallet.

Native balances, payment amounts, and fees on Zeko are displayed in **ETH**.
Zeko uses nine decimal places: 1 ETH is `1000000000` L2 base units, and each
L2 base unit maps to 1 gwei on Ethereum. The bridge performs the conversion to
Ethereum's 18-decimal wei representation. Supported ERC-20 assets use their
registered precision. A `B62…` address identifies a Zeko account; use a `0x…`
address when sending on Ethereum.

## Wallet payments

Native payments use the wallet provider directly. For MFT transfers, the web
application constructs and proves `FungibleToken.transfer` with the pinned
`mina-fungible-token@1.1.0` and `o1js@2.8.0`, then asks Auro or the MetaMask Snap
to sign the completed command through `onlySign`. This keeps the proving runtime
out of the signing Snap while preserving the wallet approval boundary. MFT
amounts are exact base units because account data does not provide trusted
decimal metadata. Because an MFT transfer carries a local proof and is
materially heavier than an ordinary bridge operation, the form defaults its MFT
fee to at least 1 ETH (`1000000000` L2 base units). The user can edit that value,
and a higher deployment-configured operation fee takes precedence.

Deposit batching, SP1 execution/proving, and Ethereum settlement submission
remain gateway/operator responsibilities. The app polls public deposit and
withdrawal endpoints; it never calls proof approval routes and never receives
`PROOF_API_KEY`.

The user-facing wallet chooser, disconnect semantics, provider preference, and
reload reconnection contract are owned by `bridge-ui/README.md`.

Protocol progress also survives reload. The app reloads deposit status from
both the gateway and Zeko bridge state, and reloads pending withdrawal requests
from the archive-backed gateway endpoint. A locally submitted withdrawal is
shown immediately while archive indexing catches up. Zeko transaction links
use `/transactions/<hash>`.

## Zeko wallet signing domain

Both supported Zeko wallets must follow the deployment signing-domain contract
in the [configuration reference](/reference/configuration#sequencer).
Wallet-facing network names are display values only. Treat any future wallet
support for custom signing salts as a versioned network migration rather than
silently changing the deployment field.

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
| `zekoArchiveGraphqlUrl` | Mina-compatible archive endpoint used by the bridge SDK; the local gateway serves it at `/archive/graphql` from the Zeko archive database. |
| `actionsApiUrl` | Public Actions GraphQL endpoint for inclusion witnesses and the canonical asset registry snapshot. |
| `expectedEthereumChainId` | `11155111` for Sepolia or `31337` for the local Anvil profile. |
| `minaSigningNetworkId` | Exact value `testnet`. |
| `auroNetworkName` | Zeko wallet display name for the custom endpoint. |
| `zekoTransactionFeeNanomina` | Fee in native L2 base units (nine decimals per ETH), supplied as a decimal string. The configuration key is retained for SDK compatibility. |
| `ethereumExplorerUrl` / `zekoExplorerUrl` | Public transaction explorer bases. |
| `pollIntervalMs` | UI status polling period. |

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

For local development, `pnpm dev` listens on `127.0.0.1:5174`. See the
[vendored SDK guide](https://github.com/zeko-labs/ethereum-settlement/blob/main/bridge-ui/vendor/README.md) for dependency provenance
and update constraints.
