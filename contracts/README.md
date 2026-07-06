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
- `src/ZekoAddress.sol` validates packed Mina/Pasta public-key encodings.

## Deployment Inputs

Deployments need:

- SP1 verifier/gateway address.
- SP1 program verification key.
- Expected Zeko Pickles verification-key hash.
- Initial Zeko settlement checkpoint values.
