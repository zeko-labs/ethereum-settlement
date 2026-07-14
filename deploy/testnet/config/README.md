This directory is populated per deployment and mounted read-only.

Required untracked files:

- `circuits.json`: the exact OCaml circuit configuration used to build the
  Zeko image and settlement fixtures. Its Ethereum holder must be the final
  bridge proxy address.
- `bridge-genesis-ledger.json`: exported by
  `tools/export-bridge-ocaml-fixtures.sh`; `bootstrap-da` posts this exact
  ledger to all three DA nodes.
- `bridge-scenario.json`: the public bridge/DA checkpoint manifest from the
  same export.
- `virtual-mina-accounts.json`: the outer account and sequencer fee-payer
  records matching the deployed settlement genesis.

Do not substitute a newly generated ledger or circuit config after the SP1
settlement ELF is built. Both are part of the proof identity.
