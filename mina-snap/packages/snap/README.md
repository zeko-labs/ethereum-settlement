# Zeko Wallet

MetaMask Snap for the Zeko Ethereum rollup. It displays your Zeko `B62…`
address, ETH balance, nonce, and custom token accounts, and signs Zeko
transactions after an explicit confirmation. Your separate Ethereum `0x…`
account handles L1 deposits and claims.

The Snap retains Auro-compatible account derivation at
`m/44'/12586'/0'/0/0` and signs through `mina-signer@4.1.0` without exporting or
persisting the private key. New installations select Zeko testnet; existing
installations preserve their selected network. Approve the deployment's
GraphQL endpoint through the bridge before querying or sending.

On the Ethereum-backed Zeko testnet, native amounts use ETH with nine decimal
places. Custom-token balances remain in exact base units when no trusted decimal
metadata is available. Mina mainnet/devnet and legacy Zeko mainnet connections
retain MINA labels.

Install this interim release from npm as `@mondejka/mina-snap`. Integrating
dapps use the source-linked `@zeko-labs/mina-snap-provider` adapter in this
repository; that adapter is bundled into the bridge and is not required at
Snap-install time.
The source, security model, compatibility matrix, local MetaMask Flask setup,
and release status are in the
[`mina-snap` workspace](https://github.com/zeko-labs/ethereum-settlement/tree/main/mina-snap).

Private-credential presentations are not supported. The Zeko Ethereum bridge
does not use that Auro feature.
