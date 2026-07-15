# Vendored Ethereum bridge SDK

The bridge SDK tarball was last built from `/root/zeko-ui` at commit
`930552298829c7cbdd12898dd8a43a584a35b3d5`, while the Ethereum SDK remains from
`26b1c8783c78aafc144ae1cc17790b61952285c5`, with:

```bash
nix develop -c pnpm exec moon run bridge-sdk:build eth-bridge-sdk:build
nix develop -c pnpm --dir packages/bridge-sdk pack
nix develop -c pnpm --dir packages/eth-bridge-sdk pack
```

SHA-256:

```text
9d1b1e6b277340d2c3624a26d2b376637ac9c1273b4e98c4df9480961d60b192  zeko-labs-bridge-sdk-0.3.4.tgz
2e280e62a02f75b6e5a88847a0be79bdf80e3dd9abee7e4d36757f018bafc72d  zeko-labs-eth-bridge-sdk-0.1.0.tgz
```

The packages must remain paired. The Ethereum SDK imports `createBridgeRuntime`
and `ethereumDepositAux`, which are present at the pinned source commit but are
missing from the previously published `@zeko-labs/bridge-sdk@0.3.4` artifact.
`scripts/check-vendored-sdk.mjs` verifies the installed package resolves to this
file tarball and exposes those symbols.
