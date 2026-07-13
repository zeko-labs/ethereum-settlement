use ark_ff::{BigInteger, PrimeField};
use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::pasta::{fp_kimchi, FULL_ROUNDS};
use mina_poseidon::permutation::poseidon_block_cipher;
use pickles_verifier::types::{StepField, VerifiableProof};
use zeko_sp1_lib::{
    Bytes32, MinaSignatureKindV1, OuterStateV1, SettlementDaMode, SettlementPublicValuesV1,
    SettlementWitnessV1,
};

const BODY_UPDATE_STATE_START: usize = 2;
const BODY_ACTIONS_HASH: usize = 15;
const BODY_PRECONDITION_STATE_START: usize = 28;
const BODY_PRECONDITION_ACTION_STATE: usize = 36;
const OUTER_COMMIT_ACTION_FIELDS: usize = 8;

pub fn derive_receipt(
    proof: &VerifiableProof,
    witness: SettlementWitnessV1,
    vk_hash: Bytes32,
) -> SettlementPublicValuesV1 {
    derive_receipt_for_app_state(&proof.app_state, witness, vk_hash)
}

fn derive_receipt_for_app_state(
    app_state: &[StepField],
    witness: SettlementWitnessV1,
    vk_hash: Bytes32,
) -> SettlementPublicValuesV1 {
    assert_eq!(
        app_state.len(),
        2,
        "Zeko settlement statement must contain body and calls digests"
    );

    let binding = witness.binding;
    let body_fields = binding
        .account_update_body
        .field_elements
        .iter()
        .map(field_from_bytes)
        .collect::<Vec<_>>();
    assert!(
        body_fields.len() > BODY_PRECONDITION_ACTION_STATE,
        "account-update body input is missing Zeko binding fields"
    );

    let mut packed_fields = Vec::new();
    let mut accumulator = StepField::from(0u8);
    let mut accumulator_bits = 0usize;
    for chunk in &binding.account_update_body.packed {
        let bits = usize::from(chunk.bits);
        assert!(bits > 0, "packed field width must be non-zero");
        assert!(
            bits < StepField::MODULUS_BIT_SIZE as usize,
            "packed field width exceeds Mina field capacity"
        );
        let value = field_from_bytes(&chunk.value);
        assert!(
            value.into_bigint().num_bits() <= bits as u32,
            "packed field does not fit declared width"
        );
        if accumulator_bits + bits < StepField::MODULUS_BIT_SIZE as usize {
            accumulator *= pow2(bits);
            accumulator += value;
            accumulator_bits += bits;
        } else {
            packed_fields.push(accumulator);
            accumulator = value;
            accumulator_bits = bits;
        }
    }
    if accumulator_bits > 0 {
        packed_fields.push(accumulator);
    }

    let mut body_hash_input = body_fields.clone();
    body_hash_input.extend_from_slice(&packed_fields);
    let body_prefix = match binding.mina_signature_kind {
        MinaSignatureKindV1::Mainnet => "MainnetZkappBody****",
        MinaSignatureKindV1::Testnet => "TestnetZkappBody****",
    };
    assert_eq!(
        hash_with_prefix(body_prefix, &body_hash_input),
        app_state[0],
        "account-update body does not match the verified Pickles statement"
    );

    let actions = binding
        .actions
        .iter()
        .map(|event| event.iter().map(field_from_bytes).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(
        actions.len(),
        1,
        "outer commit must emit exactly one action"
    );
    let action = &actions[0];
    assert_eq!(
        action.len(),
        OUTER_COMMIT_ACTION_FIELDS,
        "invalid Zeko outer commit action width"
    );
    assert_eq!(
        action[0],
        StepField::from(0u8),
        "outer action is not a commit"
    );

    let actions_hash = hash_actions(&actions);
    assert_eq!(
        actions_hash, body_fields[BODY_ACTIONS_HASH],
        "actions do not match the verified account-update body"
    );

    let state_before_fields = binding
        .state_before
        .fields
        .iter()
        .map(field_from_bytes)
        .collect::<Vec<_>>();
    // The commit rule intentionally does not precondition the pause key. The
    // other seven source fields are committed by the body; Solidity binds the
    // full source array, including pause key, to current L1 state.
    for index in 1..8 {
        assert_eq!(
            state_before_fields[index],
            body_fields[BODY_PRECONDITION_STATE_START + index],
            "source outer state does not match account preconditions"
        );
    }
    assert_eq!(
        state_before_fields[1],
        StepField::from(0u8),
        "multisig commit source must not be paused or in emergency mode"
    );

    let mut state_after_fields = state_before_fields.clone();
    for index in [1usize, 2, 3, 4, 7] {
        state_after_fields[index] = body_fields[BODY_UPDATE_STATE_START + index];
    }

    assert_eq!(action[1], state_after_fields[2], "commit ledger mismatch");
    assert_eq!(
        action[2], state_after_fields[3],
        "commit inner action state mismatch"
    );
    assert_eq!(
        action[3], state_after_fields[4],
        "commit inner action length mismatch"
    );
    assert_eq!(
        state_after_fields[1],
        StepField::from(0u8),
        "multisig commit must clear paused/emergency flags"
    );

    let outer_action_state_before = body_fields[BODY_PRECONDITION_ACTION_STATE];
    let outer_action_state_after = hash_with_prefix(
        "MinaZkappSeqEvents**",
        &[outer_action_state_before, actions_hash],
    );
    let before_length = witness.context.outer_action_state_length_before;
    let after_length = before_length
        .checked_add(1)
        .expect("outer action-state length overflow");

    SettlementPublicValuesV1 {
        da_mode: SettlementDaMode::Multisig,
        chain_id: witness.context.chain_id,
        settlement_contract: witness.context.settlement_contract,
        batch_sequence: witness.context.batch_sequence,
        vk_hash,
        app_statement: field_to_bytes(app_state[0]),
        mina_transaction_hash: witness.context.mina_transaction_hash,
        state_before: OuterStateV1 {
            fields: core::array::from_fn(|index| field_to_bytes(state_before_fields[index])),
        },
        state_after: OuterStateV1 {
            fields: core::array::from_fn(|index| field_to_bytes(state_after_fields[index])),
        },
        outer_action_state_before: field_to_bytes(outer_action_state_before),
        outer_action_state_after: field_to_bytes(outer_action_state_after),
        outer_action_state_length_before: before_length,
        outer_action_state_length_after: after_length,
        synchronized_outer_action_state: field_to_bytes(action[4]),
        synchronized_outer_action_state_length: field_to_u32(action[5]),
        slot_lower: field_to_u32(action[6]),
        slot_upper: field_to_u32(action[7]),
    }
}

fn field_from_bytes(bytes: &Bytes32) -> StepField {
    let value = StepField::from_be_bytes_mod_order(bytes);
    assert_eq!(field_to_bytes(value), *bytes, "non-canonical Mina field");
    value
}

fn field_to_bytes(value: StepField) -> Bytes32 {
    let bytes = value.into_bigint().to_bytes_be();
    assert!(bytes.len() <= 32);
    let mut output = [0u8; 32];
    output[32 - bytes.len()..].copy_from_slice(&bytes);
    output
}

fn field_to_u32(value: StepField) -> u32 {
    let bytes = field_to_bytes(value);
    assert!(
        bytes[..28].iter().all(|byte| *byte == 0),
        "field does not fit u32"
    );
    u32::from_be_bytes(bytes[28..].try_into().expect("four-byte suffix"))
}

fn pow2(bits: usize) -> StepField {
    let mut value = StepField::from(1u8);
    for _ in 0..bits {
        value += value;
    }
    value
}

fn hash_actions(actions: &[Vec<StepField>]) -> StepField {
    let mut result = empty_hash_with_prefix("MinaZkappActionsEmpty");
    for event in actions.iter().rev() {
        let event_hash = hash_with_prefix("MinaZkappEvent******", event);
        result = hash_with_prefix("MinaZkappSeqEvents**", &[result, event_hash]);
    }
    result
}

fn empty_hash_with_prefix(prefix: &str) -> StepField {
    poseidon_update([StepField::from(0u8); 3], &[prefix_to_field(prefix)])[0]
}

fn hash_with_prefix(prefix: &str, input: &[StepField]) -> StepField {
    let initial = poseidon_update([StepField::from(0u8); 3], &[prefix_to_field(prefix)]);
    poseidon_update(initial, input)[0]
}

fn poseidon_update(mut state: [StepField; 3], input: &[StepField]) -> [StepField; 3] {
    if input.is_empty() {
        poseidon_block_cipher::<StepField, PlonkSpongeConstantsKimchi, FULL_ROUNDS>(
            fp_kimchi::static_params(),
            &mut state,
        );
        return state;
    }
    for chunk in input.chunks(2) {
        state[0] += chunk[0];
        if chunk.len() == 2 {
            state[1] += chunk[1];
        }
        poseidon_block_cipher::<StepField, PlonkSpongeConstantsKimchi, FULL_ROUNDS>(
            fp_kimchi::static_params(),
            &mut state,
        );
    }
    state
}

fn prefix_to_field(prefix: &str) -> StepField {
    assert!(prefix.len() < 32, "prefix too long");
    let mut bytes = [0u8; 32];
    bytes[..prefix.len()].copy_from_slice(prefix.as_bytes());
    StepField::from_le_bytes_mod_order(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeko_sp1_lib::{
        ChunkedRandomOracleInputV1, MinaSignatureKindV1, SettlementBindingV1, SettlementContextV1,
    };

    fn field(value: u64) -> StepField {
        StepField::from(value)
    }

    fn encoded(value: u64) -> Bytes32 {
        field_to_bytes(field(value))
    }

    fn fixture() -> (Vec<StepField>, SettlementWitnessV1) {
        let mut state_before = OuterStateV1::default();
        state_before.fields = core::array::from_fn(|index| encoded(10 + index as u64));
        state_before.fields[1] = encoded(0);

        let action = vec![
            field(0),
            field(102),
            field(103),
            field(104),
            field(205),
            field(6),
            field(40),
            field(50),
        ];
        let actions = vec![action
            .iter()
            .copied()
            .map(field_to_bytes)
            .collect::<Vec<_>>()];

        let mut body = vec![StepField::from(0u8); BODY_PRECONDITION_ACTION_STATE + 1];
        body[BODY_UPDATE_STATE_START + 1] = field(0);
        body[BODY_UPDATE_STATE_START + 2] = action[1];
        body[BODY_UPDATE_STATE_START + 3] = action[2];
        body[BODY_UPDATE_STATE_START + 4] = action[3];
        body[BODY_UPDATE_STATE_START + 7] = field(107);
        body[BODY_ACTIONS_HASH] = hash_actions(&[action]);
        for index in 1..8 {
            body[BODY_PRECONDITION_STATE_START + index] =
                field_from_bytes(&state_before.fields[index]);
        }
        body[BODY_PRECONDITION_ACTION_STATE] = field(300);

        let body_digest = hash_with_prefix("TestnetZkappBody****", &body);
        let witness = SettlementWitnessV1 {
            binding: SettlementBindingV1 {
                mina_signature_kind: MinaSignatureKindV1::Testnet,
                account_update_body: ChunkedRandomOracleInputV1 {
                    field_elements: body.iter().copied().map(field_to_bytes).collect(),
                    packed: Vec::new(),
                },
                actions,
                state_before,
            },
            context: SettlementContextV1 {
                chain_id: 31337,
                settlement_contract: [0x11; 20],
                batch_sequence: 1,
                mina_transaction_hash: [0x22; 32],
                outer_action_state_length_before: 8,
            },
        };
        (vec![body_digest, field(999)], witness)
    }

    #[test]
    fn derives_complete_v1_receipt_from_bound_body_and_action() {
        let (app_state, witness) = fixture();
        let receipt = derive_receipt_for_app_state(&app_state, witness, [0x33; 32]);

        assert_eq!(receipt.chain_id, 31337);
        assert_eq!(receipt.batch_sequence, 1);
        assert_eq!(receipt.state_after.fields[0], encoded(10));
        assert_eq!(receipt.state_after.fields[2], encoded(102));
        assert_eq!(receipt.state_after.fields[3], encoded(103));
        assert_eq!(receipt.state_after.fields[4], encoded(104));
        assert_eq!(receipt.synchronized_outer_action_state, encoded(205));
        assert_eq!(receipt.synchronized_outer_action_state_length, 6);
        assert_eq!(receipt.outer_action_state_length_before, 8);
        assert_eq!(receipt.outer_action_state_length_after, 9);
        assert_eq!(receipt.slot_lower, 40);
        assert_eq!(receipt.slot_upper, 50);
        assert_eq!(
            SettlementPublicValuesV1::decode(&receipt.encode()).unwrap(),
            receipt
        );
    }

    #[test]
    #[should_panic(expected = "account-update body does not match")]
    fn rejects_body_not_bound_to_application_statement() {
        let (app_state, mut witness) = fixture();
        witness.binding.account_update_body.field_elements[2] = encoded(1234);
        let _ = derive_receipt_for_app_state(&app_state, witness, [0x33; 32]);
    }

    #[test]
    #[should_panic(expected = "actions do not match")]
    fn rejects_actions_not_bound_to_account_update_body() {
        let (app_state, mut witness) = fixture();
        witness.binding.actions[0][1] = encoded(1234);
        let _ = derive_receipt_for_app_state(&app_state, witness, [0x33; 32]);
    }
}
