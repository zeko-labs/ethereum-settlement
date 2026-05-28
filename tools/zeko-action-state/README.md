# Zeko action state fixture

Small o1js fixture that emits the same bridge deposit action payload as the SP1 bridge program.

It deploys a local o1js smart contract, calls `deposit()`, then reads and prints the Zeko action state.

```sh
cd tools/zeko-action-state
npm install
npm start
```

With the fixture values in `src/action-state.ts`, the first slot after the three sequential
`deposit()` transactions is:

```txt
0x3d638b908c4241e7b417d1790a79d0fe3277a133a5a87e12a484cd756de795bf
```

The `deposit()` method dispatches one action field:

```ts
Poseidon.hashWithPrefix("Deposit_params - qFB3jXP*)", [
  Field(0),
  ...holderAccountL1.toFields(),
  ...amount.toFields(),
  ...recipient.toFields(),
  ...timeout.toFields()
])
```
