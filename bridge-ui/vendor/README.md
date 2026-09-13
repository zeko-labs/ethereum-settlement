# Vendored Ethereum bridge SDK

All three bridge packages were built from the Zeko UI repository at commit
`a7bc7f85ebcb86d176d2a248cccd54ac45c0e618`, with:

```bash
nix develop -c pnpm exec moon run graphql:build bridge-sdk:build eth-bridge-sdk:build
nix develop -c pnpm --dir packages/graphql pack
nix develop -c pnpm --dir packages/bridge-sdk pack
nix develop -c pnpm --dir packages/eth-bridge-sdk pack
```

The authoritative SHA-256 pins and installed-package drift checks are in
[`check-vendored-sdk.mjs`](../scripts/check-vendored-sdk.mjs). To print hashes
from the artifacts themselves, run from `bridge-ui/`:

```bash
sha256sum vendor/zeko-labs-*.tgz
```

All three packages must be updated together. The Ethereum SDK and base SDK
import the canonical ERC-20 runtime and registry helpers that are present at
the pinned source commit but are missing from the previously published `@zeko-labs/bridge-sdk@0.3.4` artifact.
The check script verifies archive digests, vendored resolution of the base SDK
(including through the Ethereum SDK) and GraphQL package, and required base SDK
runtime exports.
