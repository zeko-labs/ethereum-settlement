use alloy_primitives::keccak256;
use ark_ff::{BigInteger, PrimeField};
use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::pasta::{fp_kimchi, FULL_ROUNDS};
use mina_poseidon::permutation::poseidon_block_cipher;
use pickles_verifier::types::{StepField, VerifiableProof};
use zeko_sp1_lib::{
    Address, Bytes32, InnerActionBatchWitnessV2, MinaSignatureKindV1, NativeWithdrawalV2,
    OuterStateV1, SettlementDaMode, SettlementPublicValuesV1, SettlementPublicValuesV2,
    SettlementWitnessV1, TokenWithdrawalV3,
};

const BODY_UPDATE_STATE_START: usize = 2;
const BODY_ACTIONS_HASH: usize = 15;
const BODY_PRECONDITION_STATE_START: usize = 28;
const BODY_PRECONDITION_ACTION_STATE: usize = 36;
const OUTER_COMMIT_ACTION_FIELDS: usize = 8;
const INNER_ACTION_FIELDS: usize = 3;
const INNER_ACTION_TREE_DEPTH: usize = 16;
const MAX_INNER_ACTIONS: usize = 1 << INNER_ACTION_TREE_DEPTH;

const ACTION_FIELDS_DOMAIN: &str = "ZEKO_INNER_ACTION_FIELDS_V2";
const NATIVE_WITHDRAWAL_LEAF_DOMAIN: &str = "ZEKO_NATIVE_WITHDRAWAL_LEAF_V2";
const ERC20_WITHDRAWAL_LEAF_DOMAIN: &str = "ZEKO_ERC20_WITHDRAWAL_LEAF_V3";
const RAW_INNER_ACTION_LEAF_DOMAIN: &str = "ZEKO_RAW_INNER_ACTION_LEAF_V2";
const INNER_ACTION_NODE_DOMAIN: &str = "ZEKO_INNER_ACTION_NODE_V2";

pub fn derive_receipt(
    proof: &VerifiableProof,
    witness: SettlementWitnessV1,
    vk_hash: Bytes32,
) -> SettlementPublicValuesV1 {
    derive_receipt_for_app_state(&proof.app_state, witness, vk_hash)
}

/// Derives either the byte-for-byte compatible V1 receipt or a V2 receipt when
/// the host supplies the exact inner-action range committed by the Pickles
/// proof. The clear range is safe witness data: replaying it must reach the
/// proof-bound inner action state and length before any Keccak root is emitted.
pub fn derive_receipt_bytes(
    proof: &VerifiableProof,
    witness: SettlementWitnessV1,
    vk_hash: Bytes32,
) -> Vec<u8> {
    let inner_action_batch = witness.inner_action_batch.clone();
    let v1 = derive_receipt(proof, witness, vk_hash);
    match inner_action_batch {
        None => v1.encode().to_vec(),
        Some(batch) => derive_v2_receipt(v1, batch).encode().to_vec(),
    }
}

fn derive_v2_receipt(
    settlement: SettlementPublicValuesV1,
    batch: InnerActionBatchWitnessV2,
) -> SettlementPublicValuesV2 {
    assert!(
        batch.actions.len() <= MAX_INNER_ACTIONS,
        "too many inner actions"
    );

    let state_before = field_from_bytes(settlement.state_before.inner_action_state());
    let state_after = field_from_bytes(settlement.state_after.inner_action_state());
    let start_index = field_bytes_to_u32(settlement.state_before.inner_action_state_length());
    let end_index = field_bytes_to_u32(settlement.state_after.inner_action_state_length());
    let count = u32::try_from(batch.actions.len()).expect("inner action count fits u32");
    assert_eq!(
        end_index.checked_sub(start_index),
        Some(count),
        "inner action count does not match committed length transition"
    );

    let empty_action_list_hash = empty_hash_with_prefix("MinaZkappActionsEmpty");
    let mut replayed_state = state_before;
    let mut leaves = Vec::with_capacity(batch.actions.len());

    for (offset, action) in batch.actions.iter().enumerate() {
        assert_eq!(
            action.fields.len(),
            INNER_ACTION_FIELDS,
            "invalid Zeko inner action width"
        );
        let fields = action
            .fields
            .iter()
            .map(field_from_bytes)
            .collect::<Vec<_>>();
        assert_eq!(fields[0], StepField::from(0u8), "invalid inner action tag");

        let event_hash = hash_with_prefix("MinaZkappEvent******", &fields);
        let action_list_hash = hash_with_prefix(
            "MinaZkappSeqEvents**",
            &[empty_action_list_hash, event_hash],
        );
        replayed_state =
            hash_with_prefix("MinaZkappSeqEvents**", &[replayed_state, action_list_hash]);

        let global_index = start_index
            .checked_add(u32::try_from(offset).expect("offset fits u32"))
            .expect("inner action index overflow");
        let action_fields_hash = hash_action_fields(&action.fields);
        let leaf = match (&action.withdrawal, &action.token_withdrawal) {
            (Some(withdrawal), None) => {
                assert_native_withdrawal_preimage(&fields, withdrawal);
                hash_native_withdrawal_leaf(
                    settlement.chain_id,
                    batch.bridge_address,
                    global_index,
                    withdrawal,
                    action_fields_hash,
                )
            }
            (None, Some(withdrawal)) => {
                assert_erc20_withdrawal_preimage(&fields, withdrawal);
                hash_erc20_withdrawal_leaf(
                    settlement.chain_id,
                    batch.bridge_address,
                    global_index,
                    withdrawal,
                    action_fields_hash,
                )
            }
            (None, None) => hash_raw_inner_action_leaf(
                settlement.chain_id,
                batch.bridge_address,
                global_index,
                action_fields_hash,
            ),
            (Some(_), Some(_)) => panic!("inner action has multiple withdrawal preimages"),
        };
        leaves.push(leaf);
    }

    assert_eq!(
        replayed_state, state_after,
        "inner actions do not reach proof-bound action state"
    );

    SettlementPublicValuesV2 {
        settlement,
        bridge_address: batch.bridge_address,
        inner_action_root: compute_inner_action_root(&leaves),
        inner_action_start_index: start_index,
        inner_action_count: count,
    }
}

fn assert_native_withdrawal_preimage(fields: &[StepField], withdrawal: &NativeWithdrawalV2) {
    assert!(
        withdrawal.amount > 0,
        "native withdrawal amount must be non-zero"
    );
    let recipient_x = field_from_address(withdrawal.recipient);
    // `Withdrawal_params_base.typ` fields are: empty children digest, amount,
    // compressed recipient x, compressed recipient parity. Ethereum synthetic
    // keys always use even parity.
    let expected_aux = hash_with_prefix(
        "Withdrawal_params - qFB3jXP*)",
        &[
            StepField::from(0u8),
            StepField::from(withdrawal.amount),
            recipient_x,
            StepField::from(0u8),
        ],
    );
    assert_eq!(
        fields[1], expected_aux,
        "withdrawal preimage does not match action aux"
    );
}

fn assert_erc20_withdrawal_preimage(fields: &[StepField], withdrawal: &TokenWithdrawalV3) {
    assert_ne!(
        withdrawal.token, [0u8; 20],
        "ERC20 withdrawal token is zero"
    );
    assert_ne!(
        withdrawal.asset_id, [0u8; 32],
        "ERC20 withdrawal asset id is zero"
    );
    assert!(
        withdrawal.amount > 0,
        "ERC20 withdrawal amount must be non-zero"
    );

    let params = withdrawal
        .params_fields
        .iter()
        .map(field_from_bytes)
        .collect::<Vec<_>>();
    assert!(
        params.len() >= 6,
        "ERC20 withdrawal parameter preimage is truncated"
    );

    let mut asset_high = [0u8; 32];
    asset_high[16..].copy_from_slice(&withdrawal.asset_id[..16]);
    let mut asset_low = [0u8; 32];
    asset_low[16..].copy_from_slice(&withdrawal.asset_id[16..]);
    assert_eq!(
        params[0],
        field_from_bytes(&asset_high),
        "ERC20 withdrawal asset high limb mismatch"
    );
    assert_eq!(
        params[1],
        field_from_bytes(&asset_low),
        "ERC20 withdrawal asset low limb mismatch"
    );

    let base = params.len() - 4;
    assert_eq!(
        params[base + 1],
        StepField::from(withdrawal.amount),
        "ERC20 withdrawal amount mismatch"
    );
    assert_eq!(
        params[base + 2],
        field_from_address(withdrawal.recipient),
        "ERC20 withdrawal recipient mismatch"
    );
    assert_eq!(
        params[base + 3],
        StepField::from(0u8),
        "ERC20 withdrawal recipient parity must be even"
    );

    let expected_aux = hash_with_prefix("Ethereum ERC20 withdrawal V1", &params);
    assert_eq!(
        fields[1], expected_aux,
        "ERC20 withdrawal preimage does not match action aux"
    );
}

fn hash_action_fields(fields: &[Bytes32]) -> Bytes32 {
    let mut encoded = Vec::with_capacity(64 + fields.len() * 32);
    encoded.extend_from_slice(&keccak256(ACTION_FIELDS_DOMAIN.as_bytes()).0);
    encoded.extend_from_slice(&u32_word(
        u32::try_from(fields.len()).expect("field count fits u32"),
    ));
    for field in fields {
        encoded.extend_from_slice(field);
    }
    keccak256(encoded).0
}

fn hash_native_withdrawal_leaf(
    chain_id: u64,
    bridge_address: Address,
    global_index: u32,
    withdrawal: &NativeWithdrawalV2,
    action_fields_hash: Bytes32,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(32 * 7);
    encoded.extend_from_slice(&keccak256(NATIVE_WITHDRAWAL_LEAF_DOMAIN.as_bytes()).0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge_address));
    encoded.extend_from_slice(&u32_word(global_index));
    encoded.extend_from_slice(&address_word(withdrawal.recipient));
    encoded.extend_from_slice(&u64_word(withdrawal.amount));
    encoded.extend_from_slice(&action_fields_hash);
    keccak256(encoded).0
}

fn hash_erc20_withdrawal_leaf(
    chain_id: u64,
    bridge_address: Address,
    global_index: u32,
    withdrawal: &TokenWithdrawalV3,
    action_fields_hash: Bytes32,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(32 * 9);
    encoded.extend_from_slice(&keccak256(ERC20_WITHDRAWAL_LEAF_DOMAIN.as_bytes()).0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge_address));
    encoded.extend_from_slice(&u32_word(global_index));
    encoded.extend_from_slice(&address_word(withdrawal.token));
    encoded.extend_from_slice(&withdrawal.asset_id);
    encoded.extend_from_slice(&address_word(withdrawal.recipient));
    encoded.extend_from_slice(&u64_word(withdrawal.amount));
    encoded.extend_from_slice(&action_fields_hash);
    keccak256(encoded).0
}

fn hash_raw_inner_action_leaf(
    chain_id: u64,
    bridge_address: Address,
    global_index: u32,
    action_fields_hash: Bytes32,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(32 * 5);
    encoded.extend_from_slice(&keccak256(RAW_INNER_ACTION_LEAF_DOMAIN.as_bytes()).0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge_address));
    encoded.extend_from_slice(&u32_word(global_index));
    encoded.extend_from_slice(&action_fields_hash);
    keccak256(encoded).0
}

fn compute_inner_action_root(leaves: &[Bytes32]) -> Bytes32 {
    let zero_hashes = compute_zero_hashes();
    if leaves.is_empty() {
        return zero_hashes[INNER_ACTION_TREE_DEPTH];
    }
    let mut nodes = leaves.to_vec();
    for level in 0..INNER_ACTION_TREE_DEPTH {
        let mut parents = Vec::with_capacity(nodes.len().div_ceil(2));
        for pair in nodes.chunks(2) {
            let right = if pair.len() == 2 {
                pair[1]
            } else {
                zero_hashes[level]
            };
            parents.push(hash_inner_action_node(pair[0], right));
        }
        nodes = parents;
    }
    assert_eq!(nodes.len(), 1, "invalid inner action tree");
    nodes[0]
}

fn compute_zero_hashes() -> [Bytes32; INNER_ACTION_TREE_DEPTH + 1] {
    let mut zero_hashes = [[0u8; 32]; INNER_ACTION_TREE_DEPTH + 1];
    for level in 0..INNER_ACTION_TREE_DEPTH {
        zero_hashes[level + 1] = hash_inner_action_node(zero_hashes[level], zero_hashes[level]);
    }
    zero_hashes
}

fn hash_inner_action_node(left: Bytes32, right: Bytes32) -> Bytes32 {
    let mut encoded = Vec::with_capacity(96);
    encoded.extend_from_slice(&keccak256(INNER_ACTION_NODE_DOMAIN.as_bytes()).0);
    encoded.extend_from_slice(&left);
    encoded.extend_from_slice(&right);
    keccak256(encoded).0
}

fn field_from_address(address: Address) -> StepField {
    let mut bytes = [0u8; 32];
    bytes[12..].copy_from_slice(&address);
    field_from_bytes(&bytes)
}

fn field_bytes_to_u32(value: &Bytes32) -> u32 {
    assert!(
        value[..28].iter().all(|byte| *byte == 0),
        "field does not fit u32"
    );
    u32::from_be_bytes(value[28..].try_into().expect("four-byte suffix"))
}

fn u64_word(value: u64) -> Bytes32 {
    let mut output = [0u8; 32];
    output[24..].copy_from_slice(&value.to_be_bytes());
    output
}

fn u32_word(value: u32) -> Bytes32 {
    let mut output = [0u8; 32];
    output[28..].copy_from_slice(&value.to_be_bytes());
    output
}

fn address_word(value: Address) -> Bytes32 {
    let mut output = [0u8; 32];
    output[12..].copy_from_slice(&value);
    output
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
        ChunkedRandomOracleInputV1, InnerActionWitnessV2, MinaSignatureKindV1, NativeWithdrawalV2,
        SettlementBindingV1, SettlementContextV1, TokenWithdrawalV3,
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
            inner_action_batch: None,
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
    fn v2_replays_inner_actions_and_binds_native_withdrawal_leaf() {
        let recipient = [0x22; 20];
        let withdrawal = NativeWithdrawalV2 {
            recipient,
            amount: 1_000_000_000,
        };
        let aux = hash_with_prefix(
            "Withdrawal_params - qFB3jXP*)",
            &[
                field(0),
                field(withdrawal.amount),
                field_from_address(recipient),
                field(0),
            ],
        );
        let fields = vec![field(0), aux, field(777)];
        let before = field(900);
        let event_hash = hash_with_prefix("MinaZkappEvent******", &fields);
        let list_hash = hash_with_prefix(
            "MinaZkappSeqEvents**",
            &[empty_hash_with_prefix("MinaZkappActionsEmpty"), event_hash],
        );
        let after = hash_with_prefix("MinaZkappSeqEvents**", &[before, list_hash]);

        let mut state_before = OuterStateV1::default();
        state_before.fields[3] = field_to_bytes(before);
        state_before.fields[4] = encoded(5);
        let mut state_after = state_before.clone();
        state_after.fields[3] = field_to_bytes(after);
        state_after.fields[4] = encoded(6);

        let settlement = SettlementPublicValuesV1 {
            da_mode: SettlementDaMode::Multisig,
            chain_id: 31337,
            settlement_contract: [0x11; 20],
            batch_sequence: 2,
            vk_hash: [1; 32],
            app_statement: [2; 32],
            mina_transaction_hash: [3; 32],
            state_before,
            state_after,
            outer_action_state_before: [4; 32],
            outer_action_state_after: [5; 32],
            outer_action_state_length_before: 9,
            outer_action_state_length_after: 10,
            synchronized_outer_action_state: [6; 32],
            synchronized_outer_action_state_length: 9,
            slot_lower: 1,
            slot_upper: 2,
        };
        let batch = InnerActionBatchWitnessV2 {
            bridge_address: [0x33; 20],
            actions: vec![InnerActionWitnessV2 {
                fields: fields.into_iter().map(field_to_bytes).collect(),
                withdrawal: Some(withdrawal),
                token_withdrawal: None,
            }],
        };

        let receipt = derive_v2_receipt(settlement, batch);
        assert_eq!(receipt.inner_action_start_index, 5);
        assert_eq!(receipt.inner_action_count, 1);
        assert_ne!(receipt.inner_action_root, [0; 32]);
        assert_eq!(
            SettlementPublicValuesV2::decode(&receipt.encode()).unwrap(),
            receipt
        );
    }

    #[test]
    #[should_panic(expected = "withdrawal preimage does not match action aux")]
    fn v2_rejects_unbound_withdrawal_preimage() {
        assert_native_withdrawal_preimage(
            &[field(0), field(123), field(456)],
            &NativeWithdrawalV2 {
                recipient: [0x44; 20],
                amount: 1,
            },
        );
    }

    #[test]
    fn v2_erc20_withdrawal_preimage_binds_asset_recipient_and_amount() {
        let mut asset_id = [0u8; 32];
        asset_id[15] = 1;
        asset_id[31] = 2;
        let mut recipient = [0u8; 20];
        recipient[16..].copy_from_slice(&[1, 2, 3, 4]);
        let amount = 2_000_000u64;
        let mut high = [0u8; 32];
        high[16..].copy_from_slice(&asset_id[..16]);
        let mut low = [0u8; 32];
        low[16..].copy_from_slice(&asset_id[16..]);
        let params = vec![
            field_from_bytes(&high),
            field_from_bytes(&low),
            field(0),
            field(1),
            field(1),
            field_from_bytes(&[
                0x25, 0xbe, 0xa2, 0x29, 0x10, 0xdb, 0x1c, 0xd9, 0x18, 0xa4, 0xd3, 0x66, 0xe9, 0x72,
                0x73, 0x13, 0xfe, 0xe3, 0x76, 0x01, 0x23, 0xcb, 0x90, 0x4f, 0x6f, 0x30, 0x71, 0x67,
                0x98, 0x0a, 0x51, 0x98,
            ]),
            field(0),
            field(0),
            field(amount),
            field_from_address(recipient),
            field(0),
        ];
        let aux = hash_with_prefix("Ethereum ERC20 withdrawal V1", &params);
        assert_eq!(
            field_to_bytes(aux),
            [
                0x24, 0xc5, 0x50, 0xad, 0x1d, 0x37, 0xbd, 0x87, 0x11, 0x14, 0x8b, 0x1b, 0x4a, 0xd5,
                0xf1, 0x72, 0x4c, 0x52, 0x1a, 0x62, 0xae, 0xc4, 0xb9, 0x78, 0xa1, 0xa1, 0x9a, 0x76,
                0x29, 0xbd, 0x59, 0xcf,
            ]
        );
        let fields = vec![field(0), aux, field(777)];
        let withdrawal = TokenWithdrawalV3 {
            token: [0x33; 20],
            asset_id,
            recipient,
            amount,
            params_fields: params.into_iter().map(field_to_bytes).collect(),
        };

        assert_erc20_withdrawal_preimage(&fields, &withdrawal);
        let original = hash_erc20_withdrawal_leaf(31337, [0x44; 20], 5, &withdrawal, [0x55; 32]);
        let mut other_asset = withdrawal.clone();
        other_asset.asset_id = [0x12; 32];
        let relabelled = hash_erc20_withdrawal_leaf(31337, [0x44; 20], 5, &other_asset, [0x55; 32]);
        assert_ne!(original, relabelled);
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
