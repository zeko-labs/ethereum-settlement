# MetaMask Snap for Mina signing

Research snapshot: 2026-08-13

## Conclusion

A Mina signing wallet implemented as a MetaMask Snap is feasible. A current,
exactly pinned `mina-signer` bundles and executes in the Snap SES sandbox, and a
minimal Snap test signed and verified a Mina message successfully. The practical
wallet has to contain **two separately packaged pieces**:

1. A Snap that owns key derivation, origin authorization, confirmations and
   signing.
2. A page-side Mina Provider adapter that installs/invokes the Snap through
   MetaMask and exposes the API expected by Mina dapps.

The adapter is not optional. A Snap has no DOM or page-injection capability and
cannot create `window.mina` in arbitrary sites. Dapps invoke it through
MetaMask's provider with `wallet_requestSnaps` and `wallet_invokeSnap`. This is a
platform boundary, not a missing MinaPortal feature. See MetaMask's
[execution environment](https://docs.metamask.io/snaps/learn/about-snaps/execution-environment/)
and [custom JSON-RPC API](https://docs.metamask.io/snaps/learn/about-snaps/apis/#custom-json-rpc-apis)
documentation.

The existing SotaTek `mina-portal` Snap is still listed and allowlisted in the
official MetaMask directory. Its published bundle can still perform basic Mina
message signing. It is nevertheless not a usable base as-is for a current Mina
Provider or Zeko integration: the code is dormant, its site is offline, most of
its hard-coded endpoints no longer work, and its zkApp method cannot implement
the Provider's `onlySign` contract.

The recommended course is a new, networkless signer Snap plus an adapter. Use
Auro's full Mina HD path when wallet-recovery interoperability is desired, and
reuse MinaPortal only as a historical legacy-derivation reference. Decide before
release whether MinaPortal-derived accounts must also be recovered; derivation
choices fix account identities forever.

## What “Mina Provider compatible” means today

There is not yet one finalized Mina Provider standard.

- The current de facto browser-wallet interface is
  [`@aurowallet/mina-provider@1.0.14`](https://github.com/aurowallet/mina-provider/blob/575e2b24e3f78037d7fba0098a7d27284d54ba81/src/IProvider.ts),
  with its [request/response types](https://github.com/aurowallet/mina-provider/blob/575e2b24e3f78037d7fba0098a7d27284d54ba81/src/TSTypes.ts)
  and [method names](https://github.com/aurowallet/mina-provider/blob/575e2b24e3f78037d7fba0098a7d27284d54ba81/src/constant/index.ts).
- The proposed Mina-wide interface is still an open, unnumbered
  [draft MIP](https://github.com/MinaProtocol/MIPs/blob/780daffa73f97eba59027a9f0febfe813eaf9070/MIPS/mip-draft-mina-provider-api.md)
  as of the snapshot date. It deliberately differs from Auro: for example, it
  adds `mina_signTransaction`, uses string quantities and `mina:*` network IDs,
  but omits message/field/nullifier signing.
- The older Mina Foundation
  [RFC-0008](https://github.com/MinaFoundation/Core-Grants/blob/main/RFCs/rfc-0008-wallet-provider-api.md)
  is useful history, but it is underspecified and not a final standard.

The robust target is therefore:

- implement the current Auro-shaped convenience API for existing dapps;
- implement `request({ method, params })` and the draft MIP transaction aliases;
- advertise exact capabilities and API version through `wallet_info`; and
- keep the Snap's internal RPC schema versioned so a later finalized MIP can be
  added without changing signing or derivation semantics.

Do not instantiate Auro's exported provider implementation in the adapter. That
implementation talks to the Auro extension through its own `window.postMessage`
transport. Reuse its TypeScript types, while implementing the transport with
`wallet_invokeSnap`.

### Compatibility levels

The minimum **Zeko-compatible signing provider** needs:

| Surface | Exact behavior |
| --- | --- |
| `requestAccounts()` / `mina_requestAccounts` | Prompt once per origin, persist a grant, return `string[]`. |
| `getAccounts()` / `mina_accounts` | Return the granted account list without prompting; otherwise `[]`. |
| `requestNetwork()` / `mina_requestNetwork` | Return `{ networkID: string }`, never a bare label. |
| `switchChain({ networkID })` | Select a known chain and return `{ networkID }`; emit `chainChanged`. |
| `addChain({ url, name })` | The adapter queries and validates the endpoint's `networkID`, then stores an explicit chain record and returns `{ networkID }`. |
| `sendTransaction(args)` | Accept `transaction` as a JSON string or object. With `onlySign: true`, do not submit anything and return `{ signedData: JSON.stringify(signed.data) }`. |
| Events | Implement `on`, `removeListener`, and the `removeAllListeners` helper currently used by Zeko for `chainChanged` and `accountsChanged`; also expose Auro's `networkChanged` alias. |
| Revoke | Delete the requesting origin's grant and emit `accountsChanged` with `[]`. |

This is not hypothetical. The current Zeko UI provider surface calls
`requestAccounts`, `requestNetwork`, `switchChain`, `addChain` and wallet events
in
[`packages/sdk/src/provider.ts`](https://github.com/zeko-labs/zeko-ui/blob/40f632cc9b4155b3d6c84a60669ebe964fdaa5c7/packages/sdk/src/provider.ts).
Its bridge signing flow calls
`sendTransaction({ onlySign: true, transaction })`, parses `signedData`, and
extracts `zkappCommand` in
[`apps/bridge-ui/machines/transaction.ts`](https://github.com/zeko-labs/zeko-ui/blob/40f632cc9b4155b3d6c84a60669ebe964fdaa5c7/apps/bridge-ui/machines/transaction.ts).

A provider claiming **full compatibility with the current Auro interface** must
also implement:

- payment and delegation send methods;
- string-message sign/verify;
- field sign/verify;
- structured JSON-message sign/verify;
- nullifier creation;
- `wallet_info` and all three event names;
- custom-chain management; and
- private-credential storage and presentation methods.

`mina-signer` supports transactions, strings, fields and nullifiers. It does not
implement Auro's private-credential/presentation protocol; that feature needs a
separate, audited o1js credential implementation. Until then the provider must
return a documented unsupported-method error and must not call itself fully
Auro-compatible.

## Required architecture

```text
Mina dapp / Zeko UI
        |
        | IMinaProvider methods and events
        v
page-side adapter
        |-- wallet_requestSnaps (installation/update)
        |-- wallet_invokeSnap (authorization and signing)
        `-- Mina GraphQL client (nonce/fee lookup and optional submission)
                    |
                    v
              MetaMask Snap
              - origin grants
              - deterministic Mina key
              - confirmation UI
              - mina-signer
```

Keeping node access in the adapter is preferable. The Snap then needs no
network permission, contains no endpoint allowlist and cannot confuse a signed
request with a successful submission. For `onlySign: false`, the adapter asks
the Snap to sign, submits the returned canonical signed transaction to the
selected chain, and returns `{ hash }`. Payment and delegation convenience
methods can follow the same orchestration.

The adapter first selects the actual MetaMask provider with EIP-6963; it must
not assume `window.ethereum` belongs to MetaMask when several wallet extensions
are installed. It then installs the Snap and invokes its RPC as follows:

```ts
const snapId = 'npm:@zeko/mina-wallet';

await metamaskProvider.request({
  method: 'wallet_requestSnaps',
  params: { [snapId]: { version: '1.0.0' } },
});

const result = await metamaskProvider.request({
  method: 'wallet_invokeSnap',
  params: {
    snapId,
    request: {
      method: 'mina_sendTransaction',
      params: { onlySign: true, transaction },
    },
  },
});
```

See the official
[`wallet_requestSnaps`](https://docs.metamask.io/snaps/reference/snaps-api/wallet_requestsnaps/)
and
[`wallet_invokeSnap`](https://docs.metamask.io/snaps/reference/snaps-api/wallet_invokesnap/)
references. A dapp can hold this adapter in its own wallet abstraction. Assigning
it to that dapp's `window.mina` is technically possible but risks colliding with
an installed Mina extension and still does not inject it into other sites.
MetaMask documents the recommended
[EIP-6963 discovery flow](https://docs.metamask.io/snaps/how-to/connect-to-a-snap/#detect-wallet).

The adapter should also participate in Mina wallet discovery. Current Auro
dispatches `mina:announceProvider` with frozen `{ info, provider }` detail and
re-announces when a page dispatches `mina:requestProvider`; `info` contains a
unique `slug`, `name`, data-URI `icon`, and reverse-DNS `rdns`. Mirror that
protocol with this wallet's own identity, and never claim `isAuro: true`.
Only install a `window.mina` fallback when the integrating dapp explicitly asks
for it and no provider already owns that property. See Auro's
[provider-discovery documentation](https://docs.aurowallet.com/general/wallet-basics/mina-providers)
and current
[injection implementation](https://github.com/aurowallet/auro-wallet-browser-extension/blob/65db631c1726ea640df6f2f73275347cc0fc275f/src/webHook/index.ts).
This still requires the dapp to load the adapter; a Snap cannot announce a page
provider by itself.

### Events

A standard Snap cannot push arbitrary page events. The adapter should:

- emit `accountsChanged` after connect/revoke or a selected-account change;
- emit `chainChanged` and `networkChanged` after a successful switch;
- share changes across the dapp's tabs with `BroadcastChannel`; and
- refresh `wallet_info`, accounts and network on focus or before an operation to
  detect changes made in another connected dapp.

This gives deterministic events for adapter-mediated changes. It cannot emulate
universal extension injection, and that limitation should be stated in the SDK.

## Exact Snap RPC and state model

Expose only JSON values. Validate every method and parameter at runtime before a
dialog or signing call; TypeScript casts are not validation. A compact internal
surface is easier to audit than copying every convenience method into the Snap:

| Internal method | Purpose |
| --- | --- |
| `mina_walletInfo` | `{ apiVersion, snapVersion, capabilities, derivationVersion, account, networkID }`. |
| `mina_requestAccounts` | Confirm disclosure and create an origin grant. |
| `mina_accounts` | Return accounts authorized for `origin`. |
| `mina_revokePermissions` | Remove only the calling origin's grant. |
| `mina_getNetwork` | Return the selected chain record without secrets. |
| `mina_switchChain` | Select a validated stored chain. |
| `mina_addChain` | Store adapter-validated chain metadata after a user confirmation. |
| `mina_signPayment` | Return a `mina-signer` `SignedLegacy<Payment>`. |
| `mina_signStakeDelegation` | Return `SignedLegacy<StakeDelegation>`. |
| `mina_signZkappCommand` | Return the current `mina-signer` signed object. |
| `mina_signMessage` | Return `{ data, publicKey, signature: { field, scalar } }`. |
| `mina_signFields` | Return `{ data, publicKey, signature }`. |
| `mina_createNullifier` | Return a JSON-safe nullifier with bigint fields serialized as decimal strings. |

The page adapter maps Auro method names and draft-MIP aliases onto these methods.
Verification methods do not require a private key and can run in the adapter
with `mina-signer`; keeping them out of the privileged Snap reduces its API.

Persist encrypted, schema-versioned state with `snap_manageState`, for example:

```ts
type StateV1 = {
  schema: 1;
  derivation: {
    kind: 'auro-bip44-v1' | 'minaportal-v1' | 'entropy-v1';
    accountIndex: number;
  };
  origins: Record<string, { accounts: string[] }>;
  chains: Record<string, {
    networkID: string;
    name: string;
    rpcUrl: string;
    signingDomain: 'mainnet' | 'devnet' | 'testnet' | { custom: string };
    defaultTransactionEra: 'mesa' | 'berkeley';
  }>;
  selectedNetworkID: string;
};
```

Never persist a raw private key. Snap module memory is not durable; derive the
key on demand for each privileged request. MetaMask state is encrypted by
default, but that does not make private-key export acceptable. See
[Snap data storage](https://docs.metamask.io/snaps/features/data-storage/).

Authorization rules are:

1. `origin` is supplied by MetaMask, not by request parameters.
2. `mina_requestAccounts` shows the origin and exact address, then records that
   origin's grant.
3. `mina_accounts` returns no address to an ungranted origin.
4. Signing requires a current grant and requires `from`/fee payer to equal the
   granted selected account.
5. A chain switch cannot silently change the cryptographic signing domain.
6. Revocation is scoped to the calling origin.

Reject unknown methods and unexpected object properties. Put method-specific
payload size and account-update-count limits in the schema. Use MetaMask's
standard errors (`4001`, `4100`, `4200`, `-32601`, `-32602`, `-32603`) inside the
Snap and translate them to Auro's legacy codes only at the adapter boundary.
MetaMask documents the available errors in its
[known-errors reference](https://docs.metamask.io/snaps/reference/known-errors/).

## Key derivation: the irreversible design decision

Mina uses the Pallas curve. MetaMask's
[`snap_getBip32Entropy`](https://docs.metamask.io/snaps/reference/snaps-api/snap_getbip32entropy/)
supports only `secp256k1`, `ed25519` and `ed25519Bip32`, while its current BIP-44
key-tree implementation constructs a secp256k1 node. Consequently,
[`snap_getBip44Entropy`](https://docs.metamask.io/snaps/reference/snaps-api/snap_getbip44entropy/)
does **not** directly return a Mina private key, even though Mina has SLIP-44
coin type 12586. Some reviewed bytes-to-Pallas-scalar conversion is unavoidable.

There are three defensible modes. For the requested wallet compatibility, use
the Auro-compatible mode as the default and offer MinaPortal legacy recovery
only when required.

### Auro-compatible recovery mode: recommended

Current
[Auro Wallet 2.5.2](https://github.com/aurowallet/auro-wallet-browser-extension/tree/65db631c1726ea640df6f2f73275347cc0fc275f)
derives account `i` at the full path:

```text
m/44'/12586'/i'/0/0
```

Its exact implementation is in
[`accountService.ts`](https://github.com/aurowallet/auro-wallet-browser-extension/blob/65db631c1726ea640df6f2f73275347cc0fc275f/src/background/accountService.ts#L40-L81).
The Snap can reproduce it without receiving the MetaMask recovery phrase:

1. Request `snap_getBip32Entropy` for the narrowly permitted secp256k1 prefix
   `m/44'/12586'`.
2. Rehydrate the node with `@metamask/key-tree` and derive hardened account
   `i'`, then non-hardened `0/0`.
3. Clear the top two bits of the first big-endian child private-key byte with
   `bytes[0] &= 0x3f`.
4. Reverse the bytes and Base58Check encode
   `0x5a || 0x01 || scalarLE`.
5. Derive the B62 public key with `mina-signer` and verify the pair.

This was cross-checked independently: `@metamask/key-tree@10.1.1` produced the
same secp256k1 child bytes as Auro's `@scure/bip32` derivation for a known BIP-39
seed. Applying Auro's conversion to the standard public test mnemonic
`abandon` repeated eleven times followed by `about`, account zero, produced:

```text
B62qpqCoBci3mKNrfCnLkKS2SSV9QyrPbPBABe4stVWnRRfkG8sn3t4
```

Keep this as a frozen non-secret test vector. It means a user who restores the
same BIP-39 recovery phrase in Auro and MetaMask obtains the same Mina account.
It also uses one Mina account across networks, matching Auro; network selection
changes the signature domain, not the HD coin type.

Although the entropy node is secp256k1, the result is not used for secp256k1
signing. It is deterministic seed material converted with Auro's established
mask/reverse/Mina-encoding algorithm. This conversion is custom and must be in
the audit scope.

### Snap-specific entropy mode

For a greenfield wallet that deliberately does not need recovery compatibility
with Auro, use
[`snap_getEntropy`](https://docs.metamask.io/snaps/reference/snaps-api/snap_getentropy/)
as arbitrary deterministic key material. Freeze all of the following before
release:

- npm package/Snap ID;
- entropy API version;
- domain-separated salt;
- account index encoding;
- hash/KDF and counter encoding;
- byte order and scalar rejection rules; and
- Mina Base58Check encoding.

A concrete reviewed algorithm should be specified independently of code:

1. Request 256 bits using a permanent salt such as
   `mina-wallet:key:v1:account:<uint32 index>`.
2. Hash the entropy with a fixed `MinaSnapKey/v1` domain and a counter.
3. Interpret the result as an integer and use rejection sampling until it is in
   `1..q-1`, where `q` is the Pallas scalar-field modulus. Do not use biased
   modulo reduction.
4. Encode the accepted scalar in Mina's canonical private-key Base58Check form:
   private-key version byte, internal scalar version `1`, and the 32-byte scalar
   in Mina's little-endian binable encoding.
5. Pass the resulting Base58 private key only to `mina-signer`, derive its
   public key, and erase references after the request.

The canonical encoding is implemented by o1js's current
[`PrivateKey`](https://github.com/o1-labs/o1js/blob/c05d11b8883cfd507b6569ff663225342345a417/src/mina-signer/src/curve-bigint.ts#L111-L125)
and [Base58 utility](https://github.com/o1-labs/o1js/blob/c05d11b8883cfd507b6569ff663225342345a417/src/lib/util/base58.ts).
`mina-signer` does not expose “private key from bytes,” so the Snap needs a tiny,
audited encoder or an exactly pinned Base58Check dependency. Do not deep-import
o1js/mina-signer internals.

`snap_getEntropy` is scoped to the MetaMask user entropy source, Snap ID and
salt. Renaming the npm package or changing the derivation loses the account even
with the same MetaMask recovery phrase. Publish frozen recovery test vectors and
a derivation specification before users fund an address.

### MinaPortal recovery mode

To recover the existing Snap's accounts, reproduce its scheme exactly:

- request secp256k1 BIP-32 entropy at `m/44'/12586'` on Mainnet or
  `m/44'/1'` on its test networks;
- derive hardened child `accountIndex'`, giving
  `m/44'/coinType'/accountIndex'`;
- clear the top two bits of the first big-endian private-key byte;
- reverse the 32 bytes to Mina little-endian order; and
- Base58Check encode payload `0x5a || 0x01 || scalarLE`.

This is a custom scheme. It is not the full conventional BIP-44 path
`m/44'/12586'/account'/change/address`, and its network-specific coin type means
Mainnet and Devnet use different addresses. Preserve it only as a named legacy
mode with frozen vectors.

MinaPortal also contains a real account-zero bug:
`index ? index : currentAccIndex` treats explicit index `0` as absent. Do not
copy it. Use `index ?? currentAccIndex`. The source is
[`account.ts`](https://github.com/sotatek-dev/mina-snap/blob/03657ba9b5e3ae31682e16e11371be6e1705db85/packages/snap/src/mina/account.ts#L51-L71).

The three modes produce different accounts. There is no transparent migration;
users must explicitly transfer funds. Never expose an RPC that returns or
imports a private key. Recovery should use the recovery phrase and the published
derivation, not dapp-readable secrets. Unlike `snap_getEntropy`, the Auro-style
BIP-32 mode is defined by the BIP-39 seed and path and is not tied to one Snap's
npm ID.

## `mina-signer` integration

At the snapshot date, current stable
[`mina-signer` is 4.1.0](https://www.npmjs.com/package/mina-signer). Pin it as
`"mina-signer": "4.1.0"`, not a caret range, and import only the public browser
entry point:

```ts
import Client from 'mina-signer';

const client = new Client({
  network: signingDomain,
  era: 'mesa',
});
```

The [current source](https://github.com/o1-labs/o1js/blob/c05d11b8883cfd507b6569ff663225342345a417/src/mina-signer/mina-signer.ts)
supports `mainnet`, `devnet`/deprecated `testnet`, or `{ custom: string }` network
domains, plus explicit `mesa` or legacy `berkeley` transaction eras. Pass the
era explicitly. A transaction-format prerelease already exists, so an upgrade
must be a deliberate protocol migration with new fixtures, not an automated
dependency update.

Use the dedicated methods:

```ts
const signedPayment = client.signPayment(payment, privateKey);
const signedDelegation = client.signStakeDelegation(delegation, privateKey);
const signedMessage = client.signMessage(message, privateKey);
const signedFields = client.signFields(fields.map(BigInt), privateKey);
const nullifier = client.createNullifier(fields.map(BigInt), privateKey);
const signedZkapp = client.signZkappCommand(
  { zkappCommand, feePayer },
  privateKey,
);
```

The current signer accepts integer strings for fee, amount, nonce and
`validUntil`. Internally use decimal **nanomina strings**, never JavaScript
numbers. The Auro interface still types payment amounts and fees as numbers;
the adapter should accept them for compatibility, reject non-finite or
unrepresentable values, and convert the decimal MINA value to an exact 9-decimal
nanomina string. The draft MIP's string quantities are safer.

For zkApps:

1. Accept `transaction` as a string or object and validate the complete command.
2. Determine the fee payer, fee, nonce, memo and `validUntil` from explicit
   overrides or the command; if a default is needed, the adapter obtains it
   from the selected chain before invoking the Snap.
3. Require the fee payer public key to be the granted account.
4. Decode and preserve the command memo correctly when adapting the full o1js
   JSON command into `mina-signer`'s `{ zkappCommand, feePayer }` wrapper.
5. Sign and immediately call `client.verifyZkappCommand(signed)` as a defensive
   invariant.
6. For Auro's `onlySign`, return exactly:

   ```ts
   { signedData: JSON.stringify(signed.data) }
   ```

7. Assert in tests that the signed command differs from the input only in the
   intended normalized fee-payer fields and authorization.

`mina-signer`'s exact return shape and implementation are visible in
[`signZkappCommand`](https://github.com/o1-labs/o1js/blob/c05d11b8883cfd507b6569ff663225342345a417/src/mina-signer/mina-signer.ts#L361-L390).

Network ID, endpoint and signature domain must be separate chain-record fields.
Do not infer a cryptographic domain from an arbitrary display name or URL.
Current Auro 2.5.2 maps `mina:mainnet` to `mainnet`, `zeko:mainnet` to
`{ custom: 'zeko-mainnet' }`, and its other supported networks—including
`mina:devnet` and `zeko:testnet`—to `testnet`. See Auro's
[`getSignerNetwork`](https://github.com/aurowallet/auro-wallet-browser-extension/blob/65db631c1726ea640df6f2f73275347cc0fc275f/src/background/lib/index.ts#L50-L79).
Mirror this for Auro transaction compatibility unless the target chain's
authoritative `signatureKind` says otherwise. A mismatched signature is
cryptographically invalid even when the GraphQL endpoint is correct.

Transaction era is similarly part of the chain/payload contract. Auro uses
mina-signer's default `mesa` era, but detects legacy zkApp commands by their
8-element app-state/precondition-state arrays and selects `era: 'berkeley'`;
Mesa uses 32-element state arrays. It rejects unknown or mixed lengths. Reuse
and test these rules from Auro's
[`zkAppSigner.ts`](https://github.com/aurowallet/auro-wallet-browser-extension/blob/65db631c1726ea640df6f2f73275347cc0fc275f/src/utils/zkAppSigner.ts)
rather than hard-coding Mesa for every incoming command.

## Confirmations

Every account disclosure, chain addition/switch and signing request needs an
in-Snap confirmation. The dapp's own UI is not a security boundary. Use
[`snap_dialog`](https://docs.metamask.io/snaps/reference/snaps-api/snap_dialog/)
and show:

- requesting origin;
- network ID, human name and signature domain;
- operation type and signing public key;
- payment/delegation receiver, exact amount, fee, nonce, `validUntil` and memo;
- message/fields as exact copyable data; and
- for zkApps, fee-payer details, account-update count, each target public key,
  token ID, balance change, authorization kind, actions/events/call data, plus a
  canonical payload hash for details that cannot fit safely.

Use MetaMask's `Copyable` component for untrusted strings rather than Markdown.
Reject before invoking any signing method when the dialog returns false, and
keep any request-local derived key material out of state and responses. The
official
[security guidance](https://docs.metamask.io/snaps/learn/best-practices/security-guidelines/)
also requires private keys to remain inside the Snap.

## Package, manifest and runtime requirements

A networkless, Auro-compatible signer needs this permission set:

```json
{
  "initialPermissions": {
    "endowment:rpc": { "dapps": true, "snaps": false },
    "snap_dialog": {},
    "snap_getBip32Entropy": [
      {
        "path": ["m", "44'", "12586'"],
        "curve": "secp256k1"
      }
    ],
    "snap_manageState": {}
  },
  "platformVersion": "11.2.0",
  "manifestVersion": "0.1"
}
```

For MinaPortal legacy recovery, additionally declare only the legacy
`m/44'/1'` test-network prefix. A deliberately Snap-specific build can replace
the BIP-32 permission with `snap_getEntropy`. Omit `endowment:network-access` and
`endowment:webassembly`: current `mina-signer` needs neither in the tested
configuration. `endowment:lifecycle-hooks` is optional for explicit state
migrations. Restrict `endowment:rpc` to `allowedOrigins` instead of all dapps if
this will only support Zeko sites. See MetaMask's
[permissions](https://docs.metamask.io/snaps/reference/permissions/) and
[RPC restriction](https://docs.metamask.io/snaps/how-to/restrict-rpc-api/)
documentation.

There is no developer-defined Snap CSP field. The SES sandbox and manifest
permissions are the capability boundary. The Snap is one JavaScript bundle,
with no DOM, filesystem or Node built-ins unless polyfilled. The modern
`mina-signer` bundle requires the CLI's Buffer polyfill:

```ts
export default {
  input: 'src/index.ts',
  output: { path: 'dist/bundle.js' },
  polyfills: { buffer: true },
};
```

For the stable MetaMask extension available at the snapshot date, use exact
versions compatible with its embedded Snaps platform:

```json
{
  "dependencies": {
    "@metamask/key-tree": "10.1.1",
    "@metamask/snaps-sdk": "11.2.0",
    "@noble/hashes": "1.7.1",
    "@scure/base": "2.0.0",
    "mina-signer": "4.1.0"
  },
  "devDependencies": {
    "@metamask/snaps-cli": "8.4.1",
    "@metamask/snaps-jest": "10.2.1"
  }
}
```

MetaMask extension
[`v13.43.0`](https://github.com/MetaMask/metamask-extension/releases/tag/v13.43.0)
embeds platform SDK
[`11.2.0`](https://github.com/MetaMask/metamask-extension/blob/v13.43.0/package.json#L231).
The npm SDK is already 12.0.0, but the CLI correctly warns that a manifest using
12.0.0 is ahead of the production extension. Check this again at implementation
time and pin all transitive security-sensitive dependencies through the lockfile.

Snaps remain MetaMask browser-extension functionality; MetaMask Mobile does not
support them. A product needing Mina signing on mobile needs another wallet
transport.

## Existing Mina Snap assessment

### SotaTek MinaPortal

The current artifacts are:

- repository head
  [`03657ba9b5e3ae31682e16e11371be6e1705db85`](https://github.com/sotatek-dev/mina-snap/tree/03657ba9b5e3ae31682e16e11371be6e1705db85),
  dated 2024-05-14;
- npm
  [`mina-portal@0.1.6`](https://www.npmjs.com/package/mina-portal/v/0.1.6),
  published 2024-05-13; and
- the [official directory entry](https://snaps.metamask.io/snap/npm/mina-portal/),
  showing about 1.4K installations and only version 0.1.6 allowlisted.

There are no GitHub tags/releases and the repository is dormant, though not
archived. The companion site `minaportal.sotatek.works` no longer resolves. The
published package uses `mina-signer ^3.0.7`, old Snap tooling and deep internal
imports.

Its compatibility gaps are concrete:

| Current Provider/Zeko expectation | MinaPortal 0.1.6 |
| --- | --- |
| `mina_requestAccounts` / `mina_accounts` return `string[]` | Missing; custom account-list objects instead. |
| `mina_requestNetwork -> { networkID }` | Returns bare `"Mainnet"`/`"Devnet"`. |
| zkApp transaction may be string or object | Always executes `JSON.parse(args.transaction)`; an object fails. |
| `onlySign: true -> { signedData }` | Ignores `onlySign` and always attempts submission. |
| Sign on configured supported chain | zkApp signing is hard-coded to Devnet only. |
| Provider events and permission revoke | Missing. |
| Fields, JSON messages, nullifiers, custom chains | Missing. |
| Strict parameter/error semantics | Type casts and unstructured errors. |

The relevant source is its
[RPC dispatcher](https://github.com/sotatek-dev/mina-snap/blob/03657ba9b5e3ae31682e16e11371be6e1705db85/packages/snap/src/index.ts),
[RPC list](https://github.com/sotatek-dev/mina-snap/blob/03657ba9b5e3ae31682e16e11371be6e1705db85/packages/snap/src/constants/mina-method.constant.ts),
[transaction implementation](https://github.com/sotatek-dev/mina-snap/blob/03657ba9b5e3ae31682e16e11371be6e1705db85/packages/snap/src/mina/transaction.ts)
and
[network configuration](https://github.com/sotatek-dev/mina-snap/blob/03657ba9b5e3ae31682e16e11371be6e1705db85/packages/snap/src/constants/config.constant.ts).

It also exposes `mina_exportPrivateKey` to dapps after a confirmation. A new
Snap should omit both import and export. MinaPortal's 2023
[Veridise audit](https://veridise.com/wp-content/uploads/2023/08/VAR-Mina-Snap-V101.pdf)
covered an older commit over six person-days, found 22 issues, and explicitly
called out missing RPC parameter validation. It does not validate the 2024
published bundle or a modern `mina-signer` upgrade.

### What was tested

All tests used temporary directories, fixed non-secret entropy or freshly
generated disposable keys. No funded account, transaction submission or paid
service was used.

MinaPortal source:

- `yarn install --immutable` failed because the committed lockfile disagrees
  with package manifests (`mina-signer` 3.0.0 vs requested `^3.0.7`, and o1js
  0.16.0 vs requested `^1.1.0`).
- A non-immutable resolution selected `mina-signer@3.1.0` and `o1js@1.9.1`.
- Its old SES evaluator fails under Node 24 on `Object.groupBy`, while the build
  and SES evaluation succeed under Node 18.
- The actual published 0.1.6 bundle passes `mm-snap eval` under Node 18.
- The published tarball installed in current `@metamask/snaps-jest@10.2.1`.
  `mina_accountList` returned a B62 account; the dead Mainnet endpoint produced
  its known balance-zero fallback. After confirmation, `mina_signMessage`
  succeeded and its result verified with `mina-signer@4.1.0`.
- The current Provider-shaped request
  `{ onlySign: true, transaction: <object> }` failed with
  `"[object Object]" is not valid JSON`.

Endpoint probes of MinaPortal's hard-coded URLs found:

| Endpoint purpose | Result on 2026-08-13 |
| --- | --- |
| Mainnet MinaExplorer node proxy | HTTP 403 Cloudflare response. |
| Mainnet MinaExplorer archive | DNS failure. |
| Devnet MinaScan node | HTTP 200, `SYNCED`. |
| Devnet MinaExplorer archive | DNS failure. |
| Berkeley MinaScan node | Timeout. |
| Berkeley MinaExplorer archive | DNS failure. |

MinaPortal catches some account-query failures and substitutes balance/nonce
zero, which makes its remaining network path unsafe for transaction preparation.
The isolated message test proves that the old published signing code still
executes; it does not prove an end-to-end installation or transaction flow.

A separate modern compatibility experiment under Node 20 pinned
`mina-signer@4.1.0`, `@metamask/snaps-sdk@11.2.0` and
`@metamask/snaps-cli@8.4.1`, enabled the Buffer polyfill, and used
`@metamask/snaps-jest@10.2.1` to generate a disposable key, sign a message and
verify it inside the Snap runtime. Results:

- `mm-snap build`: success;
- `mm-snap eval`: success;
- Jest Snap-runtime test: 1 passed;
- generated bundle: 165,602 bytes.

This validates the modern library/SES/message path. The final implementation
must still test MetaMask entropy, confirmations, transaction formats and a real
browser adapter.

### ChainSafe implementation

The older
[ChainSafe Mina Snap](https://github.com/ChainSafe/mina-snap/tree/95404f1820ce9788852bea3fb511652b3b6ddf44)
is not a viable base. It was last substantially updated in 2022, uses
`mina-signer ^1.1.0` and obsolete Snap APIs, supports only basic
payment/message operations, hard-codes an obsolete Devnet endpoint, and is not
in the official Snap directory.

## Test and release gate

Implementation is not complete until all of these pass:

1. Frozen derivation/recovery vectors for every account index and each enabled
   Auro, MinaPortal-legacy or Snap-specific mode.
2. Independent signature fixtures for mainnet, devnet and every Zeko signature
   domain: payment, delegation, message, fields, nullifier and zkApp.
3. Exact transaction-era fixtures, including preservation of memo,
   `validUntil`, fee, nonce, account updates and authorizations.
4. Runtime-schema negative tests for malformed Base58 keys, unsafe numbers,
   negative/overflow quantities, unknown fields, oversized payloads, wrong
   origin/account/network and conflicting fee payer.
5. Confirmation accept/reject tests proving no signing occurs before approval.
6. Per-origin grant/revoke, encrypted state migration, lock/unlock, restart and
   multi-tab event tests.
7. `mm-snap build`, `mm-snap eval`, manifest checksum validation and
   `@metamask/snaps-jest` against the built artifact.
8. Browser E2E in current MetaMask Flask using a disposable recovery phrase,
   including extension restart and the actual packed npm tarball.
9. Zeko E2E with the exact current `onlySign` zkApp payload and assertion that
   `JSON.parse(signedData).zkappCommand` verifies and completes the bridge's
   signature exchange.
10. Devnet submission tests from the adapter, separately proving `onlySign`
    never makes a network request.
11. Reproducible clean installation under the pinned Node/package-manager
    versions and an immutable lockfile.
12. Snapper, dependency review and an approved third-party audit covering the
    Snap, derivation/encoding helper and bundled `mina-signer` version.

MetaMask's [testing guide](https://docs.metamask.io/snaps/how-to/test-a-snap/)
covers Flask, sandbox and Jest. Flask must never be given a funded production
recovery phrase.

The key permissions used by a Mina Snap are protected. Stable MetaMask users
cannot install a new version until MetaMask allowlists that exact version.
Allowlisting requires public source and npm artifact, Snapper results, an
approved third-party audit of the Snap and key-management modules, remediation
of medium-or-higher findings and re-review for every release. Follow the current
[allowlisting requirements](https://docs.metamask.io/snaps/how-to/get-allowlisted/)
and [npm publishing process](https://docs.metamask.io/snaps/how-to/publish-a-snap/).

## Implementation checklist

1. Implement and document `auro-bip44-v1`; decide whether MinaPortal legacy or
   Snap-specific entropy modes are also necessary, and publish vectors.
2. Scaffold a separate Snap npm package with exact dependency pins and the
   networkless manifest above.
3. Implement derivation, state schemas, per-origin grants, runtime validators,
   standard errors and confirmation renderers.
4. Implement all `mina-signer` operations with explicit network domain and
   transaction era, followed by local verification.
5. Build a framework-independent page adapter implementing Auro's interface and
   draft-MIP aliases over `wallet_invokeSnap`.
6. Add the adapter as an explicit wallet choice in Zeko UI instead of assuming
   the only provider is injected `window.mina`.
7. Put GraphQL discovery, nonce lookup and submission in the adapter; validate
   chain IDs and signed-response hashes.
8. Run the full gate above, then audit, publish, allowlist and test the exact npm
   tarball in stable MetaMask.

The decisive feasibility result is positive: current `mina-signer` works in the
Snap runtime. The hard work is the wallet boundary—stable recovery, exact Mina
Provider semantics, transaction review, origin permissions and release
allowlisting—not the Schnorr signing call itself.

## Implementation outcome

The implementation produced from this research now lives in `mina-snap/`, with
its bridge integration in `bridge-ui/`. It uses the two-package boundary above,
with one deliberate change: GraphQL nonce lookup and optional broadcast run in
the Snap rather than the page adapter. This keeps `sendPayment`,
`sendStakeDelegation`, and `sendTransaction({ onlySign: false })` behind the
same origin grant and MetaMask confirmation as signing. The manifest therefore
declares `endowment:network-access`. A dapp-supplied endpoint is not contacted
until the user approves its displayed URL, and its returned `networkID` is
shown in a second confirmation before it is stored with that endpoint.

The checked-in conformance suite establishes the release-critical signing
claims:

- MetaMask's deterministic test phrase derives the exact Auro account-0 public
  key at `m/44'/12586'/0'/0/0`.
- Messages, JSON messages, fields, nullifiers, Mesa `onlySign`, and a
  bridge-shaped Berkeley zkApp match independent Auro/mina-signer results.
- Per-origin account grants, revoke, explicit confirmations, custom-network
  contact ordering, MetaMask/Flask EIP-6963 discovery, error translation, and
  the bridge's real `onlySign` adapter boundary are covered.
- The Snap bundle builds and evaluates in SES with a generated manifest hash;
  the provider emits publishable ESM and declarations.
- The Snap declares `endowment:page-home` and renders a wallet home page with
  the derived address, selected network, native MINA balance breakdown, nonce,
  custom token accounts, and interactive balance refresh. The page reads only
  the GraphQL endpoint already approved through the network flow and preserves
  custom-token quantities as exact base units when decimal metadata is absent.

The compatibility claim remains intentionally narrower than every feature in
Auro 2.5.2. Private credential presentation is unsupported, and stored
credential JSON is not yet validated with `mina-attestations`. Auro implements
that path in a separate sandbox using `mina-attestations@0.5.0` and
`o1js@2.4.0`; the latter's unpacked npm artifact is approximately 105 MB. It is
not needed by the Zeko Ethereum bridge and should be designed, tested, and
audited as its own proving capability instead of being folded casually into a
small signing Snap. The package README contains the exact compatibility matrix.

No npm publication, MetaMask allowlisting, Flask browser E2E, funded network
submission, or SP1 proof request was performed by this implementation. Those
remain the external release gates listed above.
