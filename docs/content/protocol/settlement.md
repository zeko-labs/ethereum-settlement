# Settlement

Settlement proves that a specific Zeko/o1 zkApp proof is valid and exposes the
state transition encoded by its first account update.

## Host preparation

The settlement host binary reads:

- a base64-encoded Zeko verification key from `proofs/vk.txt`
- a GraphQL zkApp command and proof from `proofs/graphql.txt`

The host derives the zkApp statement, computes deferred proof values, builds
the verifier index, and serializes all required inputs for the SP1 guest.

## Guest verification

`program/settlement` performs the following work inside SP1:

1. Deserializes the verification key, o1 proof, zkApp statement, deferred
   values, zkApp command, and verifier index.
2. Binds the statement to the first account update by checking the
   account-update digest and calls hash.
3. Loads the embedded Pasta SRS from `srs_rkyv.bin`.
4. Restores omitted verifier-index fields, including linearization data,
   powers of alpha, and the endomorphism constant.
5. Checks selected verifier-index commitments against the Zeko verification key.
6. Reconstructs the Kimchi public inputs and verifies the o1 proof.
7. Extracts app-state preconditions, app-state updates, and the action-state precondition.
8. Commits the result as SP1 public values.

The guest aborts if Kimchi verification fails. A successfully verified SP1
proof therefore always contains `proof_valid = true`.

## Public values

`ZkappPublicValues` is serialized in this order:

| Field | Meaning |
| --- | --- |
| `proof_valid` | Whether the Kimchi proof verified. |
| `vk_hash` | Hash of the supplied Zeko verification key. |
| `state_before[8]` | Checked app-state preconditions. Ignored slots become zero. |
| `state_after[8]` | Explicit app-state updates. Kept slots become zero. |
| `action_state_before` | Checked action-state precondition. An ignored precondition becomes zero. |

::: warning Root slot semantics
Only app-state slot `3` is interpreted as the rollup root by the Ethereum
contract. The transition must explicitly constrain `state_before[3]` and set
`state_after[3]`.
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
