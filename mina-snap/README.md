# Zeko Wallet Snap

Zeko Wallet is the MetaMask Snap for signing transactions on the Zeko Ethereum
rollup. It manages a Zeko `B62…` account, displays ETH balances on Zeko, and
works with the Ethereum bridge through an Auro-compatible browser adapter.
Your Ethereum `0x…` account pays for L1 deposits and claims; the Snap's `B62…`
account receives and sends assets on Zeko.

Zeko retains Mina-compatible account derivation and transaction signatures.
The adapter therefore keeps Auro's `window.mina` method names and result shapes,
and the bridge uses the existing `onlySign` boundary. The ETH display targets
the current Ethereum-backed `zeko:testnet` deployment. Mina mainnet/devnet and
legacy `zeko:mainnet` support retain their network names and MINA currency labels;
those networks have not been migrated by this wallet update.

The implementation is based on Auro extension 2.5.2 and its provider package,
not on the abandoned MinaPortal Snap. The detailed source review and design
record is in [`../docs/research/metamask-mina-snap.md`](../docs/research/metamask-mina-snap.md).

## Packages

- `packages/snap` derives Zeko account 0 from MetaMask, provides a wallet home
  page for the address, ETH balance, nonce, token accounts, and native payments
  on Zeko, displays confirmations, signs with `mina-signer`, and optionally
  submits signed operations to a user-approved GraphQL endpoint.
- `packages/provider` discovers MetaMask with EIP-6963, installs or reconnects
  the Snap, translates Auro method calls to `wallet_invokeSnap`, revives
  nullifier bigints, and emits Auro-style account/network events.

The Snap derives `m/44'/12586'/0'/0/0`. It reproduces Auro's conversion from
the BIP-32 secp256k1 scalar to a Mina private-key payload, but never stores or
returns that private key. Only the public address, per-origin authorization,
selected network, approved endpoints, and optional credential JSON are kept in
Snap state. Every new origin grant, signature, transaction, network change, and
credential-storage request requires a MetaMask confirmation. Reloading an
already authorized bridge origin does not open another account-permission
prompt.

## Supported wallet features

| Capability | Status |
| --- | --- |
| Accounts, revoke permission, wallet info | Compatible |
| Request, add, and switch network | Compatible |
| Message and JSON-message sign/verify | Compatible |
| Field sign/verify and nullifier creation | Compatible |
| Payment and delegation sign/broadcast | Compatible after a GraphQL endpoint is configured; delegation is rejected on Zeko like Auro |
| Standard MFT token transfer | Constructed and proved by the companion wallet UI, then approved and signed by the Snap through `onlySign` |
| zkApp sign/broadcast, including `onlySign` | Compatible; Berkeley and Mesa shapes are detected, and broadcast needs a configured endpoint |
| Auro provider errors and change events | Adapter-compatible |
| Multiple Auro account profiles | Not exposed; the Snap currently uses account 0 |
| Private credential storage | Stored per wallet account; schema/proof validation is not implemented |
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
pnpm --filter @mondejka/mina-snap start
```

Then run the bridge in another shell:

```bash
cd bridge-ui
VITE_MINA_WALLET=metamask-snap \
VITE_MINA_SNAP_ID=local:http://127.0.0.1:8080 \
pnpm dev
```

After the user chooses MetaMask Flask in the bridge's Zeko-wallet dialog, the
bridge detects Flask through EIP-6963, asks it to install the local Snap,
connects Zeko account 0, selects `zeko:testnet`, and uses the Snap for the same
`onlySign` request used with Auro. For a retained local Zeko stack, update
`bridge-ui/public/runtime-config.json` as described in the bridge UI README.

After connecting once, open MetaMask, select **Snaps**, then select **Zeko
Wallet** to view its wallet home page. New installations start on Zeko testnet;
existing installations keep their selected network. Connect through the bridge
to approve the deployment's GraphQL endpoint before reading balances or sending.
The page shows the copyable `B62…` address, exact ETH balance breakdown on Zeko,
nonce, and custom token accounts. Custom-token amounts use exact base units
because account responses do not include trusted token-decimal metadata.
The home-page form signs and broadcasts native ETH payments on Zeko through
that same approved endpoint. Use **Refresh balances** after a transaction settles.

Zeko uses nine decimal places for native ETH: `1000000000` L2 base units are
1 ETH, and one L2 base unit corresponds to 1 gwei on Ethereum. Wallet display
formatting does not change the signed amount. Approval dialogs include both
ETH and the exact L2 base units. When explicitly connected to Mina mainnet,
Mina devnet, or legacy Zeko mainnet, the Snap displays MINA instead.

The bridge application's **Wallet** tab also sends native ETH and standard MFT
tokens on Zeko. MFT construction and proving deliberately stay outside the
Snap: the standard's `FungibleToken.transfer` method requires an `o1js` proof, while
`mina-signer` supplies the fee-payer and signed-account-update authorization.
The browser builds and proves against the configured Zeko endpoint, asks the
Snap to sign the finished command through `onlySign`, then submits it. The first
MFT transfer can take several minutes while the standard token contract is
compiled. Token amounts are entered in exact base units.

When updating an existing local installation after the manifest permissions or
bundle changes, reconnect from the bridge and approve MetaMask's update prompt.
If MetaMask continues to use the cached build, remove **Zeko Wallet** from
MetaMask's Snaps settings, reload the bridge, and connect it again.

`VITE_MINA_WALLET` only seeds the bridge's initial provider preference; it never
bypasses the explicit connection chooser. `bridge-ui/README.md` owns the
selection, persistence, and disconnect behavior.

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
`npm:@mondejka/mina-snap` pinned to version `0.1.0`. This personal npm scope is
the interim release location until the Zeko Labs npm organization grants
publisher access; changing scopes later creates a distinct Snap ID and requires
a coordinated bridge configuration and MetaMask allowlisting update.

## Release boundary

The npm name and bridge default are wired. Public npm publication does not
request MetaMask allowlisting or make this protected-permission Snap available
in ordinary MetaMask. Before production use:

1. Complete an independent security review of derivation, RPC authorization,
   confirmation content, GraphQL submission, and dependency provenance.
2. Publish `packages/snap`, preserving matching package/manifest versions and
   the generated manifest shasum. The bridge currently bundles the source-linked
   provider adapter, so publishing `packages/provider` is optional and separate.
3. Test the npm Snap in Flask, submit it for MetaMask allowlisting, and pin the
   audited version in the bridge deployment.
4. Run a real browser bridge roundtrip against the deployment intended for
   release. The unit/integration gate does not submit an SP1 proof or spend
   funds.

See MetaMask's official [testing](https://docs.metamask.io/snaps/how-to/test-a-snap/),
[connection](https://docs.metamask.io/snaps/how-to/connect-to-a-snap/), and
[publishing](https://docs.metamask.io/snaps/how-to/publish-a-snap/) guidance.
