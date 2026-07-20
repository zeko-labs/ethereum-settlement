# Ethereum ↔ Zeko bridge design references

This prototype is a design study, not a production bridge implementation.

## Source references

- `apps/bridge-ui` in the companion Zeko UI repository: current Mina ↔ Zeko bridge UI structure, interactions, copy patterns, progress treatment, settings, responsive behavior, and animated contour background.
- `packages/layer` in the companion Zeko UI repository: Zeko color, spacing, typography, button, logo, and status tokens.
- The retained Mina–Zeko bridge screenshot: desktop composition and hierarchy reference.
- `packages/eth-bridge-sdk` in the companion Zeko UI repository: native Ethereum bridge client operations and the four user-facing execution steps.
- `contracts/src/EthereumZekoBridge.sol`: custody, deposit, timeout, withdrawal-delay, and claim semantics.
- `api/src/main.rs`: deposit and withdrawal progress states exposed by the gateway.

## Design decisions

- Preserve Zeko's Lexend-led typography, grayscale surfaces, warm cosmic-gold accents, contour motion, and restrained rounded geometry.
- Make the cross-wallet handoff visible because Ethereum and Zeko use different address formats and signing environments.
- Replace the Mina bridge's generic progress line with protocol-specific deposit and withdrawal steps.
- Keep proof and settlement details progressive: concise by default, expanded in the route details panel.
- Present Sepolia/testnet context explicitly because this repository is still a proof of concept.
