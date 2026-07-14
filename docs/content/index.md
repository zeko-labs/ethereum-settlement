---
layout: home

hero:
  name: "Zeko on Ethereum"
  text: "Multisig-DA settlement PoC"
  tagline: "Real OCaml Zeko transitions, verified in SP1 and checkpointed on Ethereum through a Mina-compatible gateway."
  image:
    src: /logo.svg
    alt: Zeko
  actions:
    - theme: brand
      text: Understand the system
      link: /overview
    - theme: alt
      text: Deploy the testnet PoC
      link: /operations/testnet

features:
  - title: Pickles settlement
    details: SP1 verifies the proof exported by the real OCaml Zeko committer and emits a versioned Ethereum receipt.
    link: /protocol/settlement

  - title: Native ETH bridge
    details: Finalized Ethereum deposits become exact outer Witness actions; settled inner actions become ordinary Merkle claims.
    link: /protocol/deposit-bridge

  - title: Mina-compatible gateway
    details: The sequencer keeps using the GraphQL subset it expects while the gateway owns SP1 execution, proving, submission, and indexing.
    link: /gateway/api

  - title: Operator-controlled cost
    details: Every paid Succinct request pauses after local execution until an operator approves its exact digest and price caps.
    link: /gateway/proving
---
