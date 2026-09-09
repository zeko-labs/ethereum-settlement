# Zeko Mina Wallet

MetaMask Snap for Auro-compatible Mina and Zeko signing. It derives account 0
at `m/44'/12586'/0'/0/0`, asks for an explicit confirmation before every
sensitive operation, and signs through `mina-signer@4.1.0` without exporting or
persisting the Mina private key.

Its MetaMask Snap home page displays the active network, copyable Mina address,
MINA balance and nonce, and custom token accounts from the user-approved Mina
GraphQL endpoint. Custom-token balances remain in exact base units when no
trusted decimal metadata is available.

Install this interim release from npm as `@mondejka/mina-snap`. Integrating
dapps use the source-linked `@zeko-labs/mina-snap-provider` adapter in this
repository; that adapter is bundled into the bridge and is not required at
Snap-install time.
The source, security model, compatibility matrix, local MetaMask Flask setup,
and release status are in the
[`mina-snap` workspace](https://github.com/zeko-labs/ethereum-settlement/tree/main/mina-snap).

Private-credential presentations are not supported. The Zeko Ethereum bridge
does not use that Auro feature.
