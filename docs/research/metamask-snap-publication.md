# MetaMask Snap publication and distribution

Research snapshot: 2026-08-31

## Bottom line

MetaMask currently has two builder-facing distribution locations, but three
meaningfully different release paths:

| Path | Intended use | Stock MetaMask stable |
| --- | --- | --- |
| `local:http://localhost...` | Local development with MetaMask Flask | No. Flask is an experimental developer build, and local Snap IDs are restricted to loopback hosts. |
| Public npm, open permissions only | Direct installation from a dapp with `npm:<package>` | Yes, without allowlisting, but npm publication does not create a Directory listing. |
| Public npm, protected permissions | Production Snap using any permission outside MetaMask's open list | Only after MetaMask review and version-specific allowlisting; an allowlisted Snap is then listed in the Snaps Directory. |

MetaMask documents Snaps as npm packages and the production identifier as
`npm:[packageName]`. Its current policy says that open-permission-only Snaps can
be installed without review, while third-party Snaps with protected permissions
must be allowlisted. See [Publish a Snap], [Get allowlisted], and [Install
MetaMask Flask].

There is no documented private-package, arbitrary HTTPS bundle, IPFS, GitHub
release, or self-hosted production channel for stock MetaMask. The current
implementation recognizes only `npm:` and `local:` Snap IDs, limits `local:` to
loopback, disables direct HTTP(S) and local loading by default, and disables
custom npm registries by default. A wallet host can opt into those mechanisms in
code, but stock MetaMask does not expose them as a third-party production
publication route. See the current [`SnapIdStruct` and local-host restriction],
[`detectSnapLocation` defaults], and [`NpmLocation` custom-registry guard]. An
open-permission Snap may be *unlisted* and installed by its dapp, but its package
is still public npm distribution, not private distribution.

## Public npm package requirements

Before publishing:

- `package.json.version` and `snap.manifest.json.version` must match.
- `package.json.repository.url` must identify the correct source repository.
- `snap.manifest.json.source.location.npm.packageName` must equal
  `package.json.name`.
- `proposedName` must be human-readable and must not contain “MetaMask” or
  “Snap”; the allowlisting form additionally rejects “Meta” and “Mask.”
- The manifest icon must be a valid SVG. Build the final package so the
  manifest `source.shasum` covers the exact files that will be published.
- Publish only the Snap package in a monorepo, not the workspace root. Inspect
  the tarball first (`pnpm pack --dry-run` or npm's equivalent), then test the
  published `npm:` ID in Flask. [MetaMask's publishing guide][Publish a Snap]
  specifies these checks, and the official registry CI independently recomputes
  the package checksum.

Scoped names are supported; MetaMask's own example uses a scoped npm Snap ID.
On npm, the publisher must control the user or organization scope. Scoped
packages default to private visibility, so they must be published publicly;
direct publication requires 2FA or an appropriate publishing credential. See
npm's [scoped public package guidance]. `publishConfig.access: "public"` is the
repository-friendly equivalent of passing `--access public`.

After publication, a dapp installs or reconnects with `wallet_requestSnaps`.
The optional version is an npm-style SemVer range. If the installed version
does not satisfy the range, MetaMask attempts to update to the latest satisfying
version. A user can have only one version of a Snap installed. MetaMask advises
requesting the latest compatible version unless a specific version is genuinely
required. See [`wallet_requestSnaps`] and [Connect to a Snap].

## Protected-permission review and Directory listing

For a protected-permission Snap, npm publication is necessary but not
sufficient. The current sequence is:

1. Make the Snap source publicly readable, publish the exact release to public
   npm, remove console logging, TODO comments, unused methods and permissions,
   run the Snapper scan required by MetaMask, and resolve its findings.
2. If the Snap calls a key-management method such as
   `snap_getBip32Entropy`, obtain an audit from a MetaMask-approved auditor. The
   scope must cover the code running as the Snap and all modules used for key
   management. The report must identify the audited commit and any fix commit,
   list the findings and responses, and all medium-or-higher findings must be
   addressed. The developer pays for the audit. See [Get allowlisted] and the
   official [audit policy and approved-auditor list].
3. Submit the [MetaMask Snaps Directory Information form] with the proposed
   name, builder and website, descriptions, public source and npm URLs, exact
   version, audit report when required, support/escalation details, images, and
   a demo video.
4. MetaMask reviews functionality, design, and the audit. At least two MetaMask
   approvals are required. Once allowlisted, the Snap appears in the official
   Directory; it can also be installed from a companion or integrating dapp.

The official registry shows what happens behind that review. Each verified
entry binds an `npm:` ID and each approved SemVer to a checksum, with metadata
and audit links. Its pull-request CI builds, lints and tests the registry and
runs `verify-snaps --diff`. That verifier downloads every changed npm version,
checks the latest manifest's proposed name against the registry name, checks the
registry checksum against the manifest, recomputes the checksum from the
downloaded package, and validates screenshots. After merge, MetaMask signs and
deploys the registry and triggers a Directory rebuild. See the official
[`registry schema`], [`verify-snaps` implementation], [`registry PR CI`], and
[`registry deployment workflow`]. The submission form, rather than a
builder-created registry PR, is the documented entry point.

## Versions and updates

An npm name/version pair is immutable and cannot be reused after publication.
For every Snap release:

1. choose a new SemVer;
2. set the same version in `package.json` and `snap.manifest.json`;
3. rebuild the final tarball and its manifest checksum;
4. test and publish the new public npm version; and
5. for a protected-permission Snap, submit the [Directory Information Update
   form].

Allowlisting is strict by exact version: users cannot install the new protected
version until MetaMask approves it. A fresh audit is not normally required for
each update, but the MetaMask team reviews every update and decides whether
changes require additional vetting. The update form can request removal of old
versions. See [Get allowlisted] and the [audit policy and approved-auditor
list]. npm likewise refuses reuse of an already published name/version; see
[`npm publish`].

## Application to Zeko Mina Wallet

The Snap at [`mina-snap/packages/snap`] is structurally aligned with public npm:

- package and manifest are both `0.1.0`;
- `@mondejka/mina-snap` matches the manifest package name;
- the repository URL/directory agree;
- the scope is configured with `publishConfig.access: "public"`;
- `Zeko Mina Wallet` avoids the prohibited name terms; and
- the package `files` list includes the bundle, icon, license, and manifest.

It is nevertheless a protected-permission Snap. In particular it requests
`snap_getBip32Entropy`, which independently makes the approved third-party audit
mandatory, and it requests `endowment:network-access`, which is not on the open
permission list. The audit should explicitly cover the derivation/conversion
logic, `@metamask/key-tree`, `mina-signer`, RPC origin authorization, signing
confirmations, persisted state, and the network/submission boundary. The
companion provider and bridge dapp are not automatically in MetaMask's required
audit scope, but security-critical code on which the Snap relies is.

The interim release uses the publisher's personal npm scope,
[`@mondejka/mina-snap`], because the publisher does not yet have access to the
Zeko Labs npm organization. The package remains absent from the official
[`snaps-registry`] until MetaMask completes its protected-permission review.
Moving it to a future `@zeko-labs` package creates a distinct Snap ID; the
bridge default, audit target, and allowlisting submission must move together.
The separate `@zeko-labs/mina-snap-provider` package is currently source-linked
and bundled by the bridge, so it is not required for Snap installation.

The provider pins `0.1.0` and always calls `wallet_requestSnaps`, including when
an older installation is already present. This lets MetaMask perform its normal
compatibility check and update negotiation instead of treating the mere presence
of any version as sufficient. The dapp pin must still be updated for every
intended release, and a protected Snap version must already be allowlisted before
users can install it.

Recommended release gate: freeze the audited commit; run the existing build,
tests, typecheck, `mm-snap eval`, Snapper, and a real Flask browser roundtrip;
inspect the packed files; publish and verify the public npm artifact; test the
`npm:` ID in Flask; submit the Directory form and audit; wait for two approvals
and registry deployment; then deploy the bridge against the approved version.

[Publish a Snap]: https://docs.metamask.io/snaps/how-to/publish-a-snap/
[Get allowlisted]: https://docs.metamask.io/snaps/how-to/get-allowlisted/
[Install MetaMask Flask]: https://docs.metamask.io/snaps/get-started/install-flask/
[Connect to a Snap]: https://docs.metamask.io/snaps/how-to/connect-to-a-snap/
[`wallet_requestSnaps`]: https://docs.metamask.io/snaps/reference/snaps-api/wallet_requestsnaps/
[audit policy and approved-auditor list]: https://github.com/MetaMask/snaps/wiki/Audits
[MetaMask Snaps Directory Information form]: https://go.metamask.io/snaps-directory-request
[Directory Information Update form]: https://go.metamask.io/snaps-directory-update-request
[scoped public package guidance]: https://docs.npmjs.com/creating-and-publishing-scoped-public-packages/
[`npm publish`]: https://docs.npmjs.com/cli/v11/commands/npm-publish/
[`SnapIdStruct` and local-host restriction]: https://github.com/MetaMask/snaps/blob/22be130091af3a1f5ad5555059c73deceb9534bc/packages/snaps-utils/src/snaps.ts#L244-L330
[`detectSnapLocation` defaults]: https://github.com/MetaMask/snaps/blob/22be130091af3a1f5ad5555059c73deceb9534bc/packages/snaps-controllers/src/snaps/location/location.ts#L55-L91
[`NpmLocation` custom-registry guard]: https://github.com/MetaMask/snaps/blob/22be130091af3a1f5ad5555059c73deceb9534bc/packages/snaps-controllers/src/snaps/location/npm.ts#L32-L103
[`registry schema`]: https://github.com/MetaMask/snaps-registry/blob/e7951ba640f6d8a43181dbacc2c25ae9d97a0808/src/index.ts
[`verify-snaps` implementation]: https://github.com/MetaMask/snaps-registry/blob/e7951ba640f6d8a43181dbacc2c25ae9d97a0808/scripts/verify-snaps.ts
[`registry PR CI`]: https://github.com/MetaMask/snaps-registry/blob/e7951ba640f6d8a43181dbacc2c25ae9d97a0808/.github/workflows/build-lint-test.yml
[`registry deployment workflow`]: https://github.com/MetaMask/snaps-registry/blob/e7951ba640f6d8a43181dbacc2c25ae9d97a0808/.github/workflows/publish-registry.yml
[`mina-snap/packages/snap`]: ../../mina-snap/packages/snap/package.json
[`@mondejka/mina-snap`]: https://registry.npmjs.org/%40mondejka%2Fmina-snap
[`snaps-registry`]: https://github.com/MetaMask/snaps-registry/blob/e7951ba640f6d8a43181dbacc2c25ae9d97a0808/src/registry.json
[`#ensureInstalled` implementation]: ../../mina-snap/packages/provider/src/index.ts#L146-L156
