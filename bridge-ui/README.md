# Ethereum ↔ Zeko bridge UI

Standalone React application for the native ETH bridge PoC. It talks directly
to the public gateway bridge API, gateway/sequencer GraphQL endpoints, injected
Ethereum wallets, and either Auro or the Auro-compatible MetaMask Mina Snap. It
never calls proof-operator routes.

The **Wallet** tab sends native MINA and Mina Fungible Token standard assets.
Native payments use the provider's `sendPayment` operation. Standard MFT
transfers are built and proved in the browser with the pinned
`mina-fungible-token` and `o1js` versions, signed through the same Auro-compatible
`onlySign` boundary used by bridge zkApps, and submitted to the configured
sequencer. MFT amounts use exact base units; the first proof can take several
minutes while the token contract compiles.

## Run locally

```bash
pnpm install --frozen-lockfile
pnpm dev
```

The default URL is `http://127.0.0.1:5174`. Edit
`public/runtime-config.json` before deployment; it contains public endpoints and
limits only.

Clicking **Connect Mina wallet** opens a dialog that lets each browser choose
between the MetaMask Snap (shown as MetaMask Flask for a local Snap) and Auro at
runtime. Clicking the connected Mina wallet chip opens the same account dialog
with a disconnect action. Snap disconnect attempts to revoke the bridge
origin's Mina-account permission and still clears the local session if
revocation fails; Auro disconnect is local because Auro does not expose an
equivalent dapp method. The choice is stored locally, and the settings dialog
can still change the preferred provider. Vite only seeds the initial preference
for browsers without a saved preference:

| Variable | Default | Purpose |
| --- | --- | --- |
| `VITE_MINA_WALLET` | auto | Initial preference: `auro`, `metamask-snap`, or prefer Auro when injected and otherwise the Snap; the connection dialog still requires an explicit wallet click |
| `VITE_MINA_SNAP_ID` | `npm:@zeko-labs/mina-snap` | Override with `local:http://127.0.0.1:8080` for MetaMask Flask development |

To test the local Snap, first build and serve it from `../mina-snap`, then start
this UI with the Snap as the initial preference:

```bash
cd ../mina-snap
pnpm build
pnpm --filter @zeko-labs/mina-snap start
```

In another terminal:

```bash
cd ../bridge-ui
VITE_MINA_WALLET=metamask-snap \
VITE_MINA_SNAP_ID=local:http://127.0.0.1:8080 \
pnpm dev
```

Click **Connect Mina wallet** to choose between the local Snap and Auro. Auro
must be installed and configured with the local Zeko endpoint before it can sign
against this stack.

The npm package is not usable in ordinary MetaMask until it has been published,
security-reviewed, and allowlisted. See the [Snap workspace
README](../mina-snap/README.md) for the compatibility matrix and release gate.

The checked-in file is ready for the local PoC. Operators can materialize the
same schema from environment variables before building:

For the local stack, expose Anvil to the browser on port `8546` (for example,
with `ssh -L 8546:127.0.0.1:8545 ...`). MetaMask reserves its built-in
`Localhost 8545` endpoint for chain ID 1337, while this PoC uses Anvil's chain
ID 31337.

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
| `BRIDGE_UI_ZEKO_ARCHIVE_GRAPHQL_URL` | `http://127.0.0.1:8080/archive/graphql` | Read-only Mina-compatible view rebuilt from the Zeko archive database |
| `BRIDGE_UI_ACTIONS_API_URL` | `http://127.0.0.1:9101/graphql` | Public actions preparation/index API |
| `BRIDGE_UI_ETHEREUM_CHAIN_ID` | `11155111` | Sepolia, or 31337 for the local manual stack |
| `BRIDGE_UI_AURO_NETWORK_NAME` | `Zeko Ethereum PoC` | Auro custom-network display name |
| `BRIDGE_UI_ZEKO_FEE_NANOMINA` | `2500` | Sequencer operation fee passed to the SDK |
| `BRIDGE_UI_ETHEREUM_EXPLORER_URL` | Sepolia Etherscan | Ethereum transaction links |
| `BRIDGE_UI_ZEKO_EXPLORER_URL` | Zeko testnet explorer | Zeko transaction links |
| `BRIDGE_UI_POLL_INTERVAL_MS` | `5000` | Visible-page gateway polling interval |

`minaSigningNetworkId` is intentionally not configurable. The generator and
browser validator always require `testnet`, matching Auro's current signing salt
for custom networks.

Both Zeko bridge SDK packages are vendored from the same pinned source commit.
Do not replace only one with the same-numbered registry package: the published
bridge SDK artifact lacks runtime exports required by the Ethereum wrapper.

## Mina wallet signing domain

Auro and the MetaMask Snap use the Mina signing salt `testnet` for Zeko testnet
and custom testnet networks. The PoC therefore constructs the bridge SDK runtime
with both circuit networks set to `testnet`. This is separate from the
wallet-facing chain identifier and is not the intended production network ID.

Auro 2.5.x only permits public HTTPS URLs through its dapp-facing `addChain`
method. For a local node, add `http://127.0.0.1:1923/graphql` through Auro's
Settings > Networks screen and select it manually. Auro reports the selected
wallet chain as `zeko:testnet`; that identifier is separate from the `testnet`
transaction-signing salt used by this PoC.

The Snap can add a local HTTP GraphQL endpoint after an explicit MetaMask
confirmation. It contacts the endpoint only after approval, reads `networkID`,
shows that reported ID in a second confirmation, stores the URL in Snap state,
and then applies the same signing-domain mapping.

## Validation

```bash
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
```

The fixture-free browser roundtrip is intentionally separate because it starts
from a clean full Zeko/Ethereum stack and can spend tens of minutes compiling
circuits and waiting for real OCaml commits:

```bash
BRIDGE_E2E_REUSE_STACK=true \
BRIDGE_E2E_DEPLOY_DIR=../build/manual-stack/deploy \
../tools/run-live-bridge-browser-e2e.sh
```

For an isolated run, set `BRIDGE_E2E_STACK_COMMAND` to the foreground launcher
for the prepared local deployment instead of `BRIDGE_E2E_REUSE_STACK`. The
launcher must provide Anvil chain 31337, gateway, sequencer/prover, three DA
nodes, archive relay/archive, Actions services, and disable automatic deposit
proving. Before invoking the launcher, the runner generates two Zeko signing
keys and exports them as `BRIDGE_E2E_ZEKO_PRIVATE_KEYS` plus their addresses as
`BRIDGE_E2E_ZEKO_PUBLIC_KEYS`; the launcher must add both corresponding
accounts to genesis with enough balance for deposit
finalization and one withdrawal request. Reuse mode instead requires that
variable to contain two keys which are already funded in the retained stack.
The local virtual Mina slot duration defaults to 12 seconds; override it with
`BRIDGE_E2E_VIRTUAL_MINA_SLOT_SECONDS` when the retained deployment uses a
different duration. The withdrawal step derives its time jump from the live
`claimableSlot` instead of assuming a fixed delay.
The runner builds both UIs, injects real RPC-backed wallet providers, and
retains failure screenshots and video plus a protocol-state timeline in the
Playwright report. Browser tracing is disabled for this long-running suite
because its continuous network stream can prevent Playwright from shutting
down after the test has completed. The runner never intercepts application
APIs and does not request an SP1 proof.

The production host must allow the UI origin through `API_CORS_ALLOWED_ORIGINS`
on both the gateway and sequencer-facing services. It must also serve the UI
with `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp`; o1js uses those isolation headers
for its `SharedArrayBuffer` workers.
