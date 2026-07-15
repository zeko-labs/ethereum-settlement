# Ethereum ↔ Zeko bridge design references

This prototype is a design study, not a production bridge implementation.

## Source references

- `/root/zeko-ui/apps/bridge-ui`: current Mina ↔ Zeko bridge UI structure, interactions, copy patterns, progress treatment, settings, responsive behavior, and animated contour background.
- `/root/zeko-ui/packages/layer`: Zeko color, spacing, typography, button, logo, and status tokens.
- `/root/mina-zeko-bridge-screenshot.png`: desktop composition and hierarchy reference.
- `/root/zeko-ui/packages/eth-bridge-sdk`: native Ethereum bridge client operations and the four user-facing execution steps.
- `/root/ethereum-settlement/contracts/src/EthereumZekoBridge.sol`: custody, deposit, timeout, withdrawal-delay, and claim semantics.
- `/root/ethereum-settlement/api/src/main.rs`: deposit and withdrawal progress states exposed by the gateway.

## Design decisions

- Preserve Zeko's Lexend-led typography, grayscale surfaces, warm cosmic-gold accents, contour motion, and restrained rounded geometry.
- Make the cross-wallet handoff visible because Ethereum and Zeko use different address formats and signing environments.
- Replace the Mina bridge's generic progress line with protocol-specific deposit and withdrawal steps.
- Keep proof and settlement details progressive: concise by default, expanded in the route details panel.
- Present Sepolia/testnet context explicitly because this repository is still a proof of concept.
