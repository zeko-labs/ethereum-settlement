# Vendored Ethereum bridge SDK

Both tarballs were built from `/root/zeko-ui` at commit
`26b1c8783c78aafc144ae1cc17790b61952285c5` with:

```bash
nix develop -c pnpm exec moon run bridge-sdk:build eth-bridge-sdk:build
nix develop -c pnpm --dir packages/bridge-sdk pack
nix develop -c pnpm --dir packages/eth-bridge-sdk pack
```

SHA-256:

```text
284184707b0e19d141e27e4d12c271db155e6cbd0d134581c2a48d96a929af9d  zeko-labs-bridge-sdk-0.3.4.tgz
2e280e62a02f75b6e5a88847a0be79bdf80e3dd9abee7e4d36757f018bafc72d  zeko-labs-eth-bridge-sdk-0.1.0.tgz
```

The packages must remain paired. The Ethereum SDK imports `createBridgeRuntime`
and `ethereumDepositAux`, which are present at the pinned source commit but are
missing from the previously published `@zeko-labs/bridge-sdk@0.3.4` artifact.
`scripts/check-vendored-sdk.mjs` verifies the installed package resolves to this
file tarball and exposes those symbols.
