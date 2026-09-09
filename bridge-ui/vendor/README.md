# Vendored Ethereum bridge SDK

All three bridge packages were built from the Zeko UI repository at commit
`a7bc7f85ebcb86d176d2a248cccd54ac45c0e618`, with:

```bash
nix develop -c pnpm exec moon run graphql:build bridge-sdk:build eth-bridge-sdk:build
nix develop -c pnpm --dir packages/graphql pack
nix develop -c pnpm --dir packages/bridge-sdk pack
nix develop -c pnpm --dir packages/eth-bridge-sdk pack
```

SHA-256:

```text
e0aa4a416a2888c11adbc8834d42ef1fb76fecb7bf92c2e338daf56a853e27af  zeko-labs-bridge-sdk-0.3.4.tgz
1828daa02d4277ccf34589600ce4a0ce65b86f01c451cc3fc0fd69a4a6d9fa67  zeko-labs-eth-bridge-sdk-0.1.0.tgz
afa0ec54b60b80d78237af35c560448ddbb0b792947976949f68d026a6b535b1  zeko-labs-graphql-0.3.4.tgz
```

The packages must remain paired. The Ethereum SDK and base SDK import the canonical ERC-20
runtime and registry helpers that are present at the pinned source commit but
are missing from the previously published `@zeko-labs/bridge-sdk@0.3.4` artifact.
`scripts/check-vendored-sdk.mjs` verifies the installed package resolves to this
file tarball and exposes those symbols.
