# Settlement

Settlement proves that a specific Mina/Pickles proof is valid and exposes the
settlement public values Ethereum should check.

## Host preparation

The settlement host binary reads an o1-style fixture directory:

- `vk.serde.json`
- `proof.serde.json`
- `public_input_skeleton.json`
- `app_statement.json`

The host converts those files into `pickles_verifier::types::VerifiableProof`
and writes that value to SP1 stdin. The settlement guest's verifier blob is
built at compile time from `SETTLEMENT_VK_JSON` or the default
`proofs/mainnet-blockchain-snark/vk.serde.json`.

## Guest verification

`program/settlement` performs the following work inside SP1:

1. Decodes the build-time verifier blob.
2. Reads a `VerifiableProof` from SP1 stdin.
3. Runs the o1 Pickles verifier:
   - accumulator / challenge polynomial commitment check
   - deferred-value reconstruction
   - wrap public input reconstruction
   - outer Kimchi verification
4. Commits the result as SP1 public values.

The guest aborts if any Pickles verification layer fails. A successfully
verified SP1 proof therefore always contains `proof_valid = true`.

## Public values

`ZkappPublicValues` is serialized in this order:

| Field | Meaning |
| --- | --- |
| `proof_valid` | Whether the Pickles proof verified. |
| `vk_hash` | PoC SHA-256 hash of the fixture VK JSON. Production should use the canonical OCaml/Mina verification-key hash. |
| `state_before[8]` | Placeholder zeroes in the current PoC. |
| `state_after[8]` | Placeholder zeroes in the current PoC. |
| `action_state_before` | Placeholder zero in the current PoC. |

::: warning Outer-state tracking
The real Zeko outer-state fields still need to be extracted from the OCaml
state-transition public inputs and emitted here. The current fixture verifier
only proves Pickles validity and emits the verification-key hash.
:::

## Ethereum checks

`ZekoSettlement.sol` first asks the configured SP1 verifier to verify the proof
under `programVKey`. It then checks:

```text
publicValues.proof_valid           == true
publicValues.vk_hash               == vkHash
publicValues.action_state_before   == actionState
publicValues.state_before[3]       == currentRoot
```

If all checks pass:

```text
currentRoot = publicValues.state_after[3]
```

The contract also records the accepted `action_state_before` as an indexed L2
action-state checkpoint. Withdrawal transitions use these checkpoints.

::: info Action state administration
Settlement does not derive or advance `actionState` from the proof. An account
with `ADMIN_ROLE` updates it through `setActionState`.
:::
