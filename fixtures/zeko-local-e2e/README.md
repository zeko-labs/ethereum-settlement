# Real Zeko local E2E fixture

This fixture is an OCaml-generated `Outer_rules.Rollup` Pickles proof, not an
o1 example proof. It was exported by `src/app/zeko/tests/test_all_real.ml` at
Zeko commit `6917b172ac`.

- `vk.serde.json`, `proof.serde.json`, `public_input_skeleton.json`, and
  `app_statement.json` are the four inputs consumed by the Pickles verifier.
- `settlement.json` is the value passed as the GraphQL `settlement` variable.
  It additionally binds the source outer state, account-update body preimage,
  actions, Mina transaction hash, fee payer, nonce, and command identifier.
- `statement.json` is the raw OCaml statement export retained for debugging.

The SHA-256 PoC identifier of `vk.serde.json` is:

```text
0x1681fafe59f48bd75e2d83373d9541c3450d7d6628f3c57b5c6664e7d3d2fd1d
```

The SP1 program vkey is intentionally not recorded here: it changes whenever
the guest verifier changes. Rebuild it with this fixture's VK before deploying
the local settlement contract.

To regenerate from the sibling Zeko checkout, start in the shared parent
directory:

```sh
cd zeko
ZEKO_SETTLEMENT_FIXTURE_DIR="$(realpath \
  ../ethereum-settlement/fixtures/zeko-local-e2e)" \
  nix develop "git+file://$PWD?submodules=1" --command \
  dune exec src/app/zeko/tests/test_all_real.exe
```

It performs real OCaml proving and is substantially heavier than reading or
verifying the committed fixture.
