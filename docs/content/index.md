---
layout: home

hero:
  name: "Zeko Ethereum L2"
  text: "Proof-powered settlement and bridging"
  tagline: "Verify Zeko state transitions, bridge deposits, and withdrawals on Ethereum with SP1."
  image:
    src: /logo.svg
    alt: Zeko
  actions:
    - theme: brand
      text: Explore the architecture
      link: /architecture
    - theme: alt
      text: Settlement flow
      link: /protocol/settlement

features:
  - title: Zeko Settlement
    details: Verify Zeko and o1 Kimchi proofs inside SP1, then settle the resulting rollup root on Ethereum.
    link: /protocol/settlement

  - title: Ethereum to Zeko
    details: Prove that ordered Ethereum deposits produce the expected Zeko action-state transition.
    link: /protocol/deposit-bridge

  - title: Zeko to Ethereum
    details: Prove withdrawal actions, accept settlement-backed checkpoints, and release locked assets.
    link: /protocol/withdrawals

  - title: Succinct Ethereum Verification
    details: Ethereum verifies compact SP1 proofs instead of executing Kimchi verification and action-state hashing on-chain.
    link: /reference/security-model
---
