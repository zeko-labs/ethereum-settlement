# Zeko Mina Snap Provider

Browser adapter that exposes Auro-compatible Mina Provider methods over the
Zeko Mina Wallet MetaMask Snap. It discovers MetaMask or MetaMask Flask through
EIP-6963, installs the Snap, forwards Mina RPC calls through
`wallet_invokeSnap`, revives nullifier field elements as `bigint`, maps provider
errors, and emits account/network change events.

See the [source workspace and integration
guide](https://github.com/zeko-labs/ethereum-settlement/tree/main/mina-snap).
