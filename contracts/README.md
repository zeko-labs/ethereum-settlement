# Zeko Ethereum Contracts

## Requirements

- Foundry nightly/current.
- Initialized submodules:

```sh
git submodule update --init --recursive
```

Old Foundry builds can fail while reading the OpenZeppelin submodule config.
Use `foundryup` before running local tests.

## Tests

```sh
forge build --sizes
forge test -vv
```

## Contracts

- `src/ZekoSettlement.sol` verifies SP1 settlement proofs and tracks the current
  PoC settlement root/action-state checkpoint.
- `src/EthereumZekoBridge.sol` handles Ethereum asset custody, deposit
  accumulation, withdrawal-state acceptance, and withdrawal claims.
- `src/ZekoAssetRegistry.sol` is the immutable registry module delegated
  through the bridge proxy; it owns proposal and proof-settled activation logic
  while its state remains in namespaced proxy storage.
- `src/ZekoAddress.sol` validates packed Mina/Pasta public-key encodings.
- `src/PocDeterministicFactory.sol` deploys implementations and atomically
  initialized ERC-1967 proxies at CREATE2 addresses shared by local and testnet
  environments.

## Deployment Inputs

Deployments need:

- SP1 verifier/gateway address.
- SP1 program verification key.
- Expected Zeko Pickles verification-key hash.
- Initial Zeko settlement checkpoint values.

## Deterministic PoC deployment

From the repository root, prepare all public deployment artifacts against a
running Ethereum RPC:

```sh
FORGE=$HOME/.foundry/bin/forge tools/prepare-poc.sh \
  http://127.0.0.1:8545 0x<admin>
```

This builds the gateway and settlement ELF against the selected fixture VK,
computes the settlement, bridge, and withdrawal SP1 program vkeys without
proving, predicts all CREATE2 addresses, writes `build/poc/manifest.json` and
`build/poc/deployment.env`, and sets `ZEKO_ETHEREUM_BRIDGE_ADDRESS` to the
predicted bridge proxy for the OCaml circuit config. It also derives `FORK_SLOT`
from the selected fixture's proof-bound slot range. The same admin and bytecode
produce the same proxy addresses on Anvil and Sepolia.

Deploy only after reviewing the manifest. `SP1_VERIFIER_ADDRESS` must be a real
SP1 verifier on testnet. For local contract-transition tests, set
`LOCAL_MOCK_VERIFIER=true`; the factory deploys `LocalSP1Verifier` at the
manifest's predicted address. Local mock deployments default their genesis
timestamp one day into the future so long execute-only checks remain at
`FORK_SLOT`; set `GENESIS_TIMESTAMP` to override it:

```sh
set -a
source build/poc/deployment.env
set +a
export PRIVATE_KEY=0x<admin-private-key>
export LOCAL_MOCK_VERIFIER=true

# Testnet alternative:
# export SP1_VERIFIER_ADDRESS=0x<real-sp1-verifier>

cd contracts
forge script script/DeployPoc.s.sol:DeployPoc \
  --rpc-url "$RPC_URL" --broadcast
```
