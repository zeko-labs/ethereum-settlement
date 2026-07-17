# Vendored Ethereum bridge SDK

The base bridge SDK tarball was built from the Zeko UI repository at commit
`930552298829c7cbdd12898dd8a43a584a35b3d5`; the Ethereum SDK was built from
`d1da7a6e9efc735bb0a777a7eae55cb3a32876dc`, with:

```bash
nix develop -c pnpm exec moon run bridge-sdk:build eth-bridge-sdk:build
nix develop -c pnpm --dir packages/bridge-sdk pack
nix develop -c pnpm --dir packages/eth-bridge-sdk pack
```

SHA-256:

```text
9d1b1e6b277340d2c3624a26d2b376637ac9c1273b4e98c4df9480961d60b192  zeko-labs-bridge-sdk-0.3.4.tgz
9e39f67f80cc1d287b63ba6a78bc2b70e62214292c7d69e846b73dce97260225  zeko-labs-eth-bridge-sdk-0.1.0.tgz
```

The packages must remain paired. The Ethereum SDK imports `createBridgeRuntime`
and `ethereumDepositAux`, which are present at the pinned source commit but are
missing from the previously published `@zeko-labs/bridge-sdk@0.3.4` artifact.
`scripts/check-vendored-sdk.mjs` verifies the installed package resolves to this
file tarball and exposes those symbols.
