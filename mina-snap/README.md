# Zeko Mina Wallet Snap

This workspace contains a MetaMask Snap that signs Mina and Zeko operations,
plus a browser adapter with the same dapp-facing method names and result shapes
as Auro's `window.mina` provider. The Ethereum bridge consumes that adapter at
its existing Auro `onlySign` boundary, so the bridge SDK and transaction
submission path are unchanged.

The implementation is based on Auro extension 2.5.2 and its provider package,
not on the abandoned MinaPortal Snap. The detailed source review and design
record is in [`../docs/research/metamask-mina-snap.md`](../docs/research/metamask-mina-snap.md).

## Packages

- `packages/snap` derives Mina account 0 from MetaMask, displays confirmations,
  signs with `mina-signer`, and optionally submits signed operations to a
  user-approved Mina GraphQL endpoint.
- `packages/provider` discovers MetaMask with EIP-6963, installs or reconnects
  the Snap, translates Auro method calls to `wallet_invokeSnap`, revives
  nullifier bigints, and emits Auro-style account/network events.

The Snap derives `m/44'/12586'/0'/0/0`. It reproduces Auro's conversion from
the BIP-32 secp256k1 scalar to a Mina private-key payload, but never stores or
returns that private key. Only the public address, per-origin authorization,
selected network, approved endpoints, and optional credential JSON are kept in
Snap state. Every connection, signature, transaction, network change, and
credential-storage request requires a MetaMask confirmation.

## Supported Mina Provider surface

| Capability | Status |
| --- | --- |
| Accounts, revoke permission, wallet info | Compatible |
| Request, add, and switch network | Compatible |
| Message and JSON-message sign/verify | Compatible |
| Field sign/verify and nullifier creation | Compatible |
| Payment and delegation sign/broadcast | Compatible after a GraphQL endpoint is configured; delegation is rejected on Zeko like Auro |
| zkApp sign/broadcast, including `onlySign` | Compatible; Berkeley and Mesa shapes are detected, and broadcast needs a configured endpoint |
| Auro provider errors and change events | Adapter-compatible |
| Multiple Auro account profiles | Not exposed; the Snap currently uses account 0 |
| Private credential storage | Stored per Mina account; schema/proof validation is not implemented |
| Private credential presentation | Not supported |

Credential presentation is deliberately outside the signing claim. Auro runs
`mina-attestations@0.5.0` with `o1js@2.4.0` in a separate browser sandbox to
prepare and finalize those proofs; the unpacked `o1js` dependency alone is
about 105 MB. Pulling that proving runtime into the signing Snap would greatly
expand its trusted bundle and requires a separate Snap execution/performance
design and conformance fixture. The Zeko Ethereum bridge uses none of these
credential methods.

## Install and validate

Use Node 20 or newer and the repository-pinned pnpm version:

```bash
cd mina-snap
corepack pnpm install --frozen-lockfile
pnpm build
pnpm test
pnpm typecheck
```

The Snap tests execute the compiled bundle in MetaMask's Jest simulator. Their
fixed recovery phrase assertions cover:

- the exact Auro account-0 public address;
- exact Auro signatures for messages, fields, JSON messages, nullifiers, and
  `onlySign` zkApp transactions;
- a Berkeley-era, bridge-shaped zkApp fixture whose expected fee-payer
  signature was generated independently with Auro 2.5.2's signing algorithm;
- per-origin authorization and confirmation-before-network-access behavior.

The fixture metadata records the Auro source revision and derivation path. No
private key or recovery phrase is checked into the repository.

## Try it locally

Local Snaps require MetaMask Flask. Build and serve the Snap:

```bash
cd mina-snap
corepack pnpm install --frozen-lockfile
pnpm build
pnpm --filter @zeko-labs/mina-snap start
```

Then run the bridge in another shell:

```bash
cd bridge-ui
VITE_MINA_WALLET=metamask-snap \
VITE_MINA_SNAP_ID=local:http://127.0.0.1:8080 \
pnpm dev
```

The bridge detects MetaMask through EIP-6963, asks MetaMask to install the local
Snap, connects Mina account 0, selects `zeko:testnet`, and uses the Snap for the
same `onlySign` request used with Auro. For a retained local Zeko stack, update
`bridge-ui/public/runtime-config.json` as described in the bridge UI README.

Set `VITE_MINA_WALLET=auro` to force Auro. With no override, the bridge prefers
an injected Auro provider and falls back to the MetaMask Snap. These are Vite
build-time settings.

## Dapp integration

```ts
import {
  discoverMetaMaskProvider,
  MinaSnapProvider,
} from "@zeko-labs/mina-snap-provider"

const ethereum = discoverMetaMaskProvider(window)
if (!ethereum) throw new Error("MetaMask is not installed")

const mina = new MinaSnapProvider(ethereum)
const [account] = await mina.requestAccounts()
const signed = await mina.sendTransaction({
  onlySign: true,
  transaction: JSON.stringify(zkappCommand),
})
```

For local development, pass `{ snapId:
"local:http://127.0.0.1:8080" }` to the constructor. The production default is
`npm:@zeko-labs/mina-snap` pinned to version `0.1.0`.

## Release boundary

The npm name and bridge default are wired, but this repository change does not
publish a package or request MetaMask allowlisting. Before production use:

1. Complete an independent security review of derivation, RPC authorization,
   confirmation content, GraphQL submission, and dependency provenance.
2. Publish only `packages/snap` and `packages/provider`, preserving matching
   package/manifest versions and the generated manifest shasum.
3. Test the npm Snap in Flask, submit it for MetaMask allowlisting, and pin the
   audited version in the bridge deployment.
4. Run a real browser bridge roundtrip against the deployment intended for
   release. The unit/integration gate does not submit an SP1 proof or spend
   funds.

See MetaMask's official [testing](https://docs.metamask.io/snaps/how-to/test-a-snap/),
[connection](https://docs.metamask.io/snaps/how-to/connect-to-a-snap/), and
[publishing](https://docs.metamask.io/snaps/how-to/publish-a-snap/) guidance.
