use alloy_primitives::keccak256;
use ark_ff::{BigInteger, PrimeField};
use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::pasta::{fp_kimchi, FULL_ROUNDS};
use mina_poseidon::permutation::poseidon_block_cipher;
use pickles_verifier::types::{StepField, VerifiableProof};
use zeko_sp1_lib::inner_action_commitment::{
    action_fields_hash as hash_action_fields, erc20_withdrawal_leaf as hash_erc20_withdrawal_leaf,
    native_withdrawal_leaf as hash_native_withdrawal_leaf,
    raw_inner_action_leaf as hash_raw_inner_action_leaf, root as compute_inner_action_root,
    MAX_LEAVES as MAX_INNER_ACTIONS,
};
use zeko_sp1_lib::{
    Address, AssetRegistryAppendV1, AssetRegistryBatchCheckpointV4, AssetRegistryCheckpointV3,
    Bytes32, CallForestNodeV3, CanonicalAssetRecordV1, ChunkedRandomOracleInputV1,
    InnerActionBatchWitnessV2, MinaSignatureKindV1, NativeWithdrawalV2, OuterStateV1,
    SettlementDaMode, SettlementPublicValuesV1, SettlementPublicValuesV2, SettlementPublicValuesV3,
    SettlementPublicValuesV4, SettlementWitnessV1, TokenWithdrawalV3, ERC20_ACTION_ENCODING_V1,
    ERC20_ACTION_ENCODING_V2,
};

const BODY_UPDATE_STATE_START: usize = 2;
const BODY_ACTIONS_HASH: usize = 15;
const BODY_CALL_DATA: usize = 16;
const BODY_PRECONDITION_STATE_START: usize = 28;
const BODY_PRECONDITION_ACTION_STATE: usize = 36;
const OUTER_COMMIT_ACTION_FIELDS: usize = 8;
const INNER_ACTION_FIELDS: usize = 3;
const ASSET_REGISTRY_TREE_DEPTH: usize = 8;
const MAX_ASSET_RECORDS: usize = 1 << ASSET_REGISTRY_TREE_DEPTH;
const ACCOUNT_UPDATE_NODE_PREFIX: &str = "MinaAcctUpdateNode**";
const ACCOUNT_UPDATE_CONS_PREFIX: &str = "MinaAcctUpdateCons**";

const ASSET_RECORD_BATCH_LEAF_DOMAIN: &str = "ZEKO_ASSET_RECORD_BATCH_LEAF_V2";
const ASSET_RECORD_BATCH_NODE_DOMAIN: &str = "ZEKO_ASSET_RECORD_BATCH_NODE_V1";
// Must match Zeko's Pickles-bound sequencer child call-data commitment.
const ASSET_REGISTRY_CHECKPOINT_DOMAIN: &str = "Zeko registry checkpoint V1";

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
    let chain_id = witness.context.chain_id;
    let inner_action_batch = witness.inner_action_batch.clone();
    let asset_registry_checkpoint = witness.asset_registry_checkpoint.clone();
    let asset_registry_batch = witness.asset_registry_batch.clone();
    let v1 = derive_receipt(proof, witness, vk_hash);
    match (
        inner_action_batch,
        asset_registry_checkpoint,
        asset_registry_batch,
    ) {
        (None, None, None) => v1.encode().to_vec(),
        (Some(batch), None, None) => derive_v2_receipt(v1, batch).encode().to_vec(),
        (Some(batch), Some(checkpoint), None) => {
            validate_registry_transition(&checkpoint, chain_id, batch.bridge_address);
            assert_ne!(checkpoint.root, [0u8; 32], "asset registry root is zero");
            assert!(checkpoint.count > 0, "asset registry count is zero");
            assert_eq!(
                checkpoint.schema_version, 1,
                "unsupported asset registry schema"
            );
            assert_ne!(
                checkpoint.record_hash, [0u8; 32],
                "asset registry record hash is zero"
            );
            SettlementPublicValuesV3 {
                settlement: derive_v2_receipt(v1, batch),
                asset_registry_root: checkpoint.root,
                asset_registry_count: checkpoint.count,
                asset_registry_schema_version: checkpoint.schema_version,
                asset_record_hash: checkpoint.record_hash,
                asset_record_commitment: field_to_bytes(hash_registry_record_leaf(
                    &checkpoint.record,
                )),
            }
            .encode()
            .to_vec()
        }
        (Some(batch), None, Some(checkpoint)) => {
            let record_identities =
                validate_registry_batch(&checkpoint, chain_id, batch.bridge_address);
            let asset_record_batch_root = asset_record_batch_root(&record_identities);
            SettlementPublicValuesV4 {
                settlement: derive_v2_receipt(v1, batch),
                asset_registry_root: checkpoint.root,
                asset_registry_count: checkpoint.count,
                asset_registry_schema_version: checkpoint.schema_version,
                asset_record_batch_root,
                asset_record_batch_count: u32::try_from(record_identities.len())
                    .expect("asset record batch length fits u32"),
            }
            .encode()
            .to_vec()
        }
        (None, Some(_), None) | (None, None, Some(_)) => {
            panic!("asset registry checkpoint requires a V2 inner-action batch")
        }
        (_, Some(_), Some(_)) => {
            panic!("V3 and V4 asset registry checkpoints are mutually exclusive")
        }
    }
}

fn validate_registry_batch(
    checkpoint: &AssetRegistryBatchCheckpointV4,
    chain_id: u64,
    bridge_address: Address,
) -> Vec<(u32, Bytes32, Bytes32)> {
    assert_eq!(
        checkpoint.schema_version, 1,
        "unsupported asset registry schema"
    );
    assert_ne!(checkpoint.root, [0u8; 32], "asset registry root is zero");
    assert!(
        !checkpoint.appends.is_empty(),
        "asset registry batch is empty"
    );
    assert!(
        checkpoint.appends.len() <= MAX_ASSET_RECORDS,
        "asset registry batch exceeds capacity"
    );
    let append_count =
        u32::try_from(checkpoint.appends.len()).expect("asset registry batch length fits u32");
    assert_eq!(
        checkpoint.count,
        checkpoint
            .old_count
            .checked_add(append_count)
            .expect("asset registry count overflow"),
        "asset registry count does not match batch length"
    );
    assert!(
        usize::try_from(checkpoint.count).expect("u32 fits usize") <= MAX_ASSET_RECORDS,
        "asset registry count exceeds capacity"
    );

    let mut running_root = checkpoint.old_root;
    let mut record_identities = Vec::with_capacity(checkpoint.appends.len());
    for (offset, append) in checkpoint.appends.iter().enumerate() {
        let expected_index = checkpoint
            .old_count
            .checked_add(u32::try_from(offset).expect("batch offset fits u32"))
            .expect("asset registry index overflow");
        running_root = validate_registry_append(
            append,
            expected_index,
            running_root,
            checkpoint.schema_version,
            chain_id,
            bridge_address,
        );
        record_identities.push((
            expected_index,
            hash_canonical_asset_record(&append.record),
            field_to_bytes(hash_registry_record_leaf(&append.record)),
        ));
    }
    assert_eq!(
        running_root, checkpoint.root,
        "registry appends do not produce the settled root"
    );
    record_identities
}

fn validate_registry_append(
    append: &AssetRegistryAppendV1,
    expected_index: u32,
    old_root: Bytes32,
    schema_version: u32,
    chain_id: u64,
    bridge_address: Address,
) -> Bytes32 {
    let record = &append.record;
    assert_eq!(
        append.append_path.len(),
        ASSET_REGISTRY_TREE_DEPTH,
        "invalid registry append path"
    );
    assert_eq!(
        record.schema_version, schema_version,
        "asset and registry schema mismatch"
    );
    assert_eq!(
        record.registry_index, expected_index,
        "asset record is not a dense ordered append"
    );
    validate_canonical_asset_record(record, chain_id, bridge_address);
    let implied_old_root =
        registry_implied_root(StepField::from(0u8), expected_index, &append.append_path);
    assert_eq!(
        field_to_bytes(implied_old_root),
        old_root,
        "registry append slot is not empty under the running root"
    );
    field_to_bytes(registry_implied_root(
        hash_registry_record_leaf(record),
        expected_index,
        &append.append_path,
    ))
}

fn validate_registry_transition(
    checkpoint: &AssetRegistryCheckpointV3,
    chain_id: u64,
    bridge_address: Address,
) {
    let record = &checkpoint.record;
    assert_eq!(
        checkpoint.append_path.len(),
        8,
        "invalid registry append path"
    );
    assert_eq!(
        record.schema_version, 1,
        "unsupported canonical asset schema"
    );
    assert_eq!(
        record.schema_version, checkpoint.schema_version,
        "asset and registry schema mismatch"
    );
    assert_eq!(
        record.registry_index, checkpoint.old_count,
        "asset record is not appended at the committed count"
    );
    assert_eq!(
        checkpoint.count,
        checkpoint
            .old_count
            .checked_add(1)
            .expect("asset registry count overflow"),
        "asset registry count must increase exactly once"
    );
    assert!(
        usize::try_from(checkpoint.count).expect("u32 fits usize") <= MAX_ASSET_RECORDS,
        "asset registry count exceeds capacity"
    );
    validate_canonical_asset_record(record, chain_id, bridge_address);
    assert_eq!(
        hash_canonical_asset_record(record),
        checkpoint.record_hash,
        "canonical asset record hash mismatch"
    );

    let old_root = registry_implied_root(
        StepField::from(0u8),
        record.registry_index,
        &checkpoint.append_path,
    );
    assert_eq!(
        field_to_bytes(old_root),
        checkpoint.old_root,
        "registry append slot is not empty under the old root"
    );
    let new_root = registry_implied_root(
        hash_registry_record_leaf(record),
        record.registry_index,
        &checkpoint.append_path,
    );
    assert_eq!(
        field_to_bytes(new_root),
        checkpoint.root,
        "registry append does not produce the settled root"
    );
}

fn validate_canonical_asset_record(
    record: &CanonicalAssetRecordV1,
    chain_id: u64,
    bridge_address: Address,
) {
    assert!(
        record.decimals <= 9,
        "asset decimals exceed the Zeko maximum"
    );
    assert_ne!(record.ethereum_token, [0u8; 20], "asset token is zero");
    assert_ne!(record.asset_id, [0u8; 32], "asset ID is zero");
    assert_ne!(record.token_owner_l2, [0u8; 32], "asset owner is zero");
    assert_ne!(record.token_id_l2, [0u8; 32], "asset token ID is zero");
    assert_ne!(record.inventory_cap, 0, "asset inventory cap is zero");
    assert_ne!(
        record.mft_standard_vk_id, [0u8; 32],
        "asset MFT standard VK is zero"
    );
    assert_ne!(
        record.vault_public_key, [0u8; 32],
        "asset vault public key is zero"
    );
    assert_ne!(
        record.universal_bridge_vk_id, [0u8; 32],
        "asset universal bridge VK is zero"
    );
    assert_ne!(
        record.token_owner_l2, record.vault_public_key,
        "asset owner and shared vault must differ"
    );
    assert_eq!(
        compute_asset_id(record, chain_id, bridge_address),
        record.asset_id,
        "canonical Ethereum asset ID mismatch"
    );
}

fn asset_record_batch_root(record_identities: &[(u32, Bytes32, Bytes32)]) -> Bytes32 {
    let mut level = vec![[0u8; 32]; MAX_ASSET_RECORDS];
    for (index, record_hash, record_commitment) in record_identities {
        let index = usize::try_from(*index).expect("asset registry index fits usize");
        assert!(
            index < MAX_ASSET_RECORDS,
            "asset registry index exceeds capacity"
        );
        assert_eq!(
            level[index], [0u8; 32],
            "duplicate asset record batch index"
        );
        let mut leaf = Vec::with_capacity(96);
        leaf.extend_from_slice(&keccak256(ASSET_RECORD_BATCH_LEAF_DOMAIN.as_bytes()).0);
        leaf.extend_from_slice(record_hash);
        leaf.extend_from_slice(record_commitment);
        level[index] = keccak256(leaf).0;
    }
    for _ in 0..ASSET_REGISTRY_TREE_DEPTH {
        level = level
            .chunks_exact(2)
            .map(|pair| {
                let mut node = Vec::with_capacity(96);
                node.extend_from_slice(&keccak256(ASSET_RECORD_BATCH_NODE_DOMAIN.as_bytes()).0);
                node.extend_from_slice(&pair[0]);
                node.extend_from_slice(&pair[1]);
                keccak256(node).0
            })
            .collect();
    }
    level[0]
}

fn compute_asset_id(
    record: &CanonicalAssetRecordV1,
    chain_id: u64,
    bridge_address: Address,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(32 * 7);
    encoded.extend_from_slice(&keccak256("ZEKO_ERC20_ASSET_V1".as_bytes()).0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge_address));
    encoded.extend_from_slice(&address_word(record.ethereum_token));
    encoded.extend_from_slice(&record.token_owner_l2);
    encoded.extend_from_slice(&record.token_id_l2);
    encoded.extend_from_slice(&u64_word(u64::from(record.decimals)));
    keccak256(encoded).0
}

fn hash_canonical_asset_record(record: &CanonicalAssetRecordV1) -> Bytes32 {
    let mut encoded = Vec::with_capacity(32 * 12);
    encoded.extend_from_slice(&keccak256("ZEKO_ERC20_ASSET_RECORD_V1".as_bytes()).0);
    encoded.extend_from_slice(&u32_word(record.schema_version));
    encoded.extend_from_slice(&u32_word(record.registry_index));
    encoded.extend_from_slice(&record.asset_id);
    encoded.extend_from_slice(&address_word(record.ethereum_token));
    encoded.extend_from_slice(&record.token_owner_l2);
    encoded.extend_from_slice(&record.token_id_l2);
    encoded.extend_from_slice(&u64_word(u64::from(record.decimals)));
    encoded.extend_from_slice(&u64_word(record.inventory_cap));
    encoded.extend_from_slice(&record.mft_standard_vk_id);
    encoded.extend_from_slice(&record.vault_public_key);
    encoded.extend_from_slice(&record.universal_bridge_vk_id);
    keccak256(encoded).0
}

fn unpack_public_key(packed: Bytes32) -> (StepField, StepField) {
    let is_odd = packed[0] & 0x80 != 0;
    let mut x = packed;
    x[0] &= 0x7f;
    (field_from_bytes(&x), StepField::from(u8::from(is_odd)))
}

fn hash_registry_record_leaf(record: &CanonicalAssetRecordV1) -> StepField {
    let mut asset_high = [0u8; 32];
    asset_high[16..].copy_from_slice(&record.asset_id[..16]);
    let mut asset_low = [0u8; 32];
    asset_low[16..].copy_from_slice(&record.asset_id[16..]);
    let (owner_x, owner_is_odd) = unpack_public_key(record.token_owner_l2);
    let (vault_x, vault_is_odd) = unpack_public_key(record.vault_public_key);
    hash_with_prefix(
        "Ethereum asset registry leaf V1",
        &[
            StepField::from(record.schema_version),
            StepField::from(record.registry_index),
            field_from_bytes(&asset_high),
            field_from_bytes(&asset_low),
            field_from_address(record.ethereum_token),
            owner_x,
            owner_is_odd,
            field_from_bytes(&record.token_id_l2),
            StepField::from(record.decimals),
            StepField::from(record.inventory_cap),
            field_from_bytes(&record.mft_standard_vk_id),
            vault_x,
            vault_is_odd,
            field_from_bytes(&record.universal_bridge_vk_id),
        ],
    )
}

fn registry_implied_root(leaf: StepField, index: u32, path: &[Bytes32]) -> StepField {
    path.iter()
        .enumerate()
        .fold(leaf, |current, (level, sibling)| {
            let sibling = field_from_bytes(sibling);
            if index & (1u32 << level) == 0 {
                hash_with_prefix("Ethereum asset registry node V1", &[current, sibling])
            } else {
                hash_with_prefix("Ethereum asset registry node V1", &[sibling, current])
            }
        })
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
    let (asset_offset, prefix) = match withdrawal.encoding_version {
        ERC20_ACTION_ENCODING_V1 => {
            assert_eq!(
                withdrawal.registry_index, 0,
                "legacy ERC20 withdrawal has a registry index"
            );
            assert_eq!(
                withdrawal.record_commitment, [0u8; 32],
                "legacy ERC20 withdrawal has a record commitment"
            );
            (0, "Ethereum ERC20 withdrawal V1")
        }
        ERC20_ACTION_ENCODING_V2 => {
            assert!(
                params.len() >= 9,
                "registry ERC20 withdrawal parameter preimage is truncated"
            );
            assert_ne!(
                withdrawal.record_commitment, [0u8; 32],
                "registry ERC20 withdrawal record commitment is zero"
            );
            assert_eq!(
                params[0],
                StepField::from(ERC20_ACTION_ENCODING_V2),
                "ERC20 withdrawal encoding version mismatch"
            );
            assert_eq!(
                params[1],
                StepField::from(withdrawal.registry_index),
                "ERC20 withdrawal registry index mismatch"
            );
            assert_eq!(
                params[2],
                field_from_bytes(&withdrawal.record_commitment),
                "ERC20 withdrawal record commitment mismatch"
            );
            (3, "Ethereum ERC20 withdrawal V2")
        }
        version => panic!("unsupported ERC20 withdrawal encoding version {version}"),
    };
    assert_eq!(
        params[asset_offset],
        field_from_bytes(&asset_high),
        "ERC20 withdrawal asset high limb mismatch"
    );
    assert_eq!(
        params[asset_offset + 1],
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

    let expected_aux = hash_with_prefix(prefix, &params);
    assert_eq!(
        fields[1], expected_aux,
        "ERC20 withdrawal preimage does not match action aux"
    );
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

    let registry_checkpoint = witness.asset_registry_checkpoint.clone();
    let registry_batch = witness.asset_registry_batch.clone();
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

    assert_eq!(
        hash_account_update_body(binding.mina_signature_kind, &binding.account_update_body),
        app_state[0],
        "account-update body does not match the verified Pickles statement"
    );
    if registry_checkpoint.is_some() || registry_batch.is_some() {
        assert_eq!(
            hash_call_forest(binding.mina_signature_kind, &binding.call_forest),
            app_state[1],
            "child call forest does not match the verified Pickles statement"
        );
    }
    if let Some(checkpoint) = registry_checkpoint {
        let matching_registry_calls = count_registry_checkpoint_calls(
            &binding.call_forest,
            checkpoint.registry_public_key,
            checkpoint.root,
            checkpoint.count,
            checkpoint.schema_version,
        );
        assert_eq!(
            matching_registry_calls, 1,
            "registry checkpoint must match exactly one authenticated child call"
        );
    }
    if let Some(checkpoint) = registry_batch {
        let matching_registry_calls = count_registry_checkpoint_calls(
            &binding.call_forest,
            checkpoint.registry_public_key,
            checkpoint.root,
            checkpoint.count,
            checkpoint.schema_version,
        );
        assert_eq!(
            matching_registry_calls, 1,
            "registry batch must match exactly one authenticated child call"
        );
    }

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

fn hash_account_update_body(
    signature_kind: MinaSignatureKindV1,
    input: &ChunkedRandomOracleInputV1,
) -> StepField {
    let mut hash_input = input
        .field_elements
        .iter()
        .map(field_from_bytes)
        .collect::<Vec<_>>();
    let mut packed_fields = Vec::new();
    let mut accumulator = StepField::from(0u8);
    let mut accumulator_bits = 0usize;
    for chunk in &input.packed {
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
    hash_input.extend_from_slice(&packed_fields);
    let prefix = match signature_kind {
        MinaSignatureKindV1::Mainnet => "MainnetZkappBody****",
        MinaSignatureKindV1::Testnet => "TestnetZkappBody****",
    };
    hash_with_prefix(prefix, &hash_input)
}

fn hash_call_forest(signature_kind: MinaSignatureKindV1, forest: &[CallForestNodeV3]) -> StepField {
    forest
        .iter()
        .rev()
        .fold(StepField::from(0u8), |tail, node| {
            let account_update =
                hash_account_update_body(signature_kind, &node.account_update_body);
            let calls = hash_call_forest(signature_kind, &node.calls);
            hash_call_forest_node(account_update, calls, tail)
        })
}

fn hash_call_forest_node(
    account_update: StepField,
    calls: StepField,
    tail: StepField,
) -> StepField {
    let tree = hash_with_prefix(ACCOUNT_UPDATE_NODE_PREFIX, &[account_update, calls]);
    hash_with_prefix(ACCOUNT_UPDATE_CONS_PREFIX, &[tree, tail])
}

fn count_registry_checkpoint_calls(
    forest: &[CallForestNodeV3],
    registry_public_key: Bytes32,
    root: Bytes32,
    count: u32,
    schema_version: u32,
) -> usize {
    // The registry is an L2 account, so its checkpoint cannot be an executable
    // Mina L1 account precondition. The signed sequencer child commits this
    // digest in inert call data, and the verified call-forest hash authenticates
    // the child body.
    let expected_call_data = field_to_bytes(hash_with_prefix(
        ASSET_REGISTRY_CHECKPOINT_DOMAIN,
        &[
            field_from_bytes(&registry_public_key),
            field_from_bytes(&root),
            StepField::from(count),
            StepField::from(schema_version),
        ],
    ));
    forest
        .iter()
        .map(|node| {
            let fields = &node.account_update_body.field_elements;
            let matches =
                fields.len() > BODY_CALL_DATA && fields[BODY_CALL_DATA] == expected_call_data;
            usize::from(matches)
                + count_registry_checkpoint_calls(
                    &node.calls,
                    registry_public_key,
                    root,
                    count,
                    schema_version,
                )
        })
        .sum()
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
    use core::str::FromStr;
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

    #[test]
    fn call_forest_node_matches_ocaml_registration_vector() {
        let account_update = StepField::from_str(
            "6528279222021746619597853333770653270120181231726695154482993200072609572798",
        )
        .unwrap();
        let actual =
            hash_call_forest_node(account_update, StepField::from(0u8), StepField::from(0u8));
        let expected = StepField::from_str(
            "6713979864511449168695265977170256483008903802405252410767630545091817987004",
        )
        .unwrap();
        assert_eq!(actual, expected);
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
                call_forest: Vec::new(),
            },
            context: SettlementContextV1 {
                chain_id: 31337,
                settlement_contract: [0x11; 20],
                batch_sequence: 1,
                mina_transaction_hash: [0x22; 32],
                outer_action_state_length_before: 8,
            },
            inner_action_batch: None,
            asset_registry_checkpoint: None,
            asset_registry_batch: None,
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
        let registry_index = 7u32;
        let record_commitment = encoded(991);
        let params = vec![
            field(ERC20_ACTION_ENCODING_V2.into()),
            field(u64::from(registry_index)),
            field_from_bytes(&record_commitment),
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
        let aux = hash_with_prefix("Ethereum ERC20 withdrawal V2", &params);
        let fields = vec![field(0), aux, field(777)];
        let withdrawal = TokenWithdrawalV3 {
            encoding_version: ERC20_ACTION_ENCODING_V2,
            registry_index,
            record_commitment,
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
        let mut other_index = withdrawal.clone();
        other_index.registry_index += 1;
        assert_ne!(
            original,
            hash_erc20_withdrawal_leaf(31337, [0x44; 20], 5, &other_index, [0x55; 32])
        );
        let mut other_commitment = withdrawal;
        other_commitment.record_commitment = encoded(992);
        assert_ne!(
            original,
            hash_erc20_withdrawal_leaf(31337, [0x44; 20], 5, &other_commitment, [0x55; 32])
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

    #[test]
    fn registry_checkpoint_is_bound_to_the_verified_child_call_digest() {
        let (mut app_state, mut witness) = fixture();
        let registry_key = field_to_bytes(StepField::from(991u64));
        let root = field_to_bytes(StepField::from(992u64));
        let mut child_fields = vec![[0u8; 32]; BODY_PRECONDITION_ACTION_STATE + 1];
        child_fields[0] = encoded(777);
        child_fields[BODY_CALL_DATA] = field_to_bytes(hash_with_prefix(
            ASSET_REGISTRY_CHECKPOINT_DOMAIN,
            &[
                field_from_bytes(&registry_key),
                field_from_bytes(&root),
                StepField::from(1u32),
                StepField::from(1u32),
            ],
        ));
        witness.binding.call_forest = vec![CallForestNodeV3 {
            account_update_body: ChunkedRandomOracleInputV1 {
                field_elements: child_fields,
                packed: Vec::new(),
            },
            calls: Vec::new(),
        }];
        witness.asset_registry_checkpoint = Some(AssetRegistryCheckpointV3 {
            registry_public_key: registry_key,
            root,
            count: 1,
            schema_version: 1,
            record_hash: [0x44; 32],
            record: CanonicalAssetRecordV1 {
                schema_version: 1,
                registry_index: 0,
                asset_id: [0x11; 32],
                ethereum_token: [0x22; 20],
                token_owner_l2: field_to_bytes(StepField::from(101u64)),
                token_id_l2: field_to_bytes(StepField::from(102u64)),
                decimals: 9,
                inventory_cap: 1_000_000,
                mft_standard_vk_id: field_to_bytes(StepField::from(103u64)),
                vault_public_key: field_to_bytes(StepField::from(104u64)),
                universal_bridge_vk_id: field_to_bytes(StepField::from(105u64)),
            },
            append_path: vec![[0u8; 32]; 8],
            old_root: [0u8; 32],
            old_count: 0,
        });
        app_state[1] = hash_call_forest(MinaSignatureKindV1::Testnet, &witness.binding.call_forest);

        let _receipt = derive_receipt_for_app_state(&app_state, witness.clone(), [0x55; 32]);

        witness
            .asset_registry_checkpoint
            .as_mut()
            .expect("checkpoint")
            .root = field_to_bytes(StepField::from(993u64));
        assert!(std::panic::catch_unwind(|| {
            derive_receipt_for_app_state(&app_state, witness, [0x55; 32])
        })
        .is_err());
    }

    #[test]
    fn canonical_record_hash_and_poseidon_append_are_bound_together() {
        let chain_id = 31_337;
        let bridge_address = [0x33; 20];
        let path = vec![[0u8; 32]; 8];
        let mut record = CanonicalAssetRecordV1 {
            schema_version: 1,
            registry_index: 0,
            asset_id: [0u8; 32],
            ethereum_token: [0x22; 20],
            token_owner_l2: field_to_bytes(StepField::from(101u64)),
            token_id_l2: field_to_bytes(StepField::from(102u64)),
            decimals: 9,
            inventory_cap: 1_000_000,
            mft_standard_vk_id: field_to_bytes(StepField::from(103u64)),
            vault_public_key: field_to_bytes(StepField::from(104u64)),
            universal_bridge_vk_id: field_to_bytes(StepField::from(105u64)),
        };
        record.asset_id = compute_asset_id(&record, chain_id, bridge_address);
        let checkpoint = AssetRegistryCheckpointV3 {
            registry_public_key: field_to_bytes(StepField::from(106u64)),
            root: field_to_bytes(registry_implied_root(
                hash_registry_record_leaf(&record),
                0,
                &path,
            )),
            count: 1,
            schema_version: 1,
            record_hash: hash_canonical_asset_record(&record),
            record,
            append_path: path.clone(),
            old_root: field_to_bytes(registry_implied_root(StepField::from(0u8), 0, &path)),
            old_count: 0,
        };
        validate_registry_transition(&checkpoint, chain_id, bridge_address);

        let mut drifted = checkpoint.clone();
        drifted.record.inventory_cap += 1;
        assert!(std::panic::catch_unwind(|| {
            validate_registry_transition(&drifted, chain_id, bridge_address)
        })
        .is_err());
        let mut unsupported_decimals = drifted;
        unsupported_decimals.record.decimals = 10;
        assert!(std::panic::catch_unwind(|| {
            validate_registry_transition(&unsupported_decimals, chain_id, bridge_address)
        })
        .is_err());

        let mut over_capacity = checkpoint;
        over_capacity.old_count = MAX_ASSET_RECORDS as u32;
        over_capacity.count = MAX_ASSET_RECORDS as u32 + 1;
        over_capacity.record.registry_index = MAX_ASSET_RECORDS as u32;
        over_capacity.record.asset_id =
            compute_asset_id(&over_capacity.record, chain_id, bridge_address);
        over_capacity.record_hash = hash_canonical_asset_record(&over_capacity.record);
        over_capacity.old_root = field_to_bytes(registry_implied_root(
            StepField::from(0u8),
            over_capacity.record.registry_index,
            &over_capacity.append_path,
        ));
        over_capacity.root = field_to_bytes(registry_implied_root(
            hash_registry_record_leaf(&over_capacity.record),
            over_capacity.record.registry_index,
            &over_capacity.append_path,
        ));
        assert!(std::panic::catch_unwind(|| {
            validate_registry_transition(&over_capacity, chain_id, bridge_address)
        })
        .is_err());
    }

    #[test]
    fn two_record_registry_batch_binds_dense_poseidon_appends_and_exact_hashes() {
        let chain_id = 31_337;
        let bridge_address = [0x33; 20];
        let mut zero_hashes = vec![StepField::from(0u8)];
        for level in 0..ASSET_REGISTRY_TREE_DEPTH {
            zero_hashes.push(hash_with_prefix(
                "Ethereum asset registry node V1",
                &[zero_hashes[level], zero_hashes[level]],
            ));
        }
        let first_path = zero_hashes[..ASSET_REGISTRY_TREE_DEPTH]
            .iter()
            .copied()
            .map(field_to_bytes)
            .collect::<Vec<_>>();
        let mut first = CanonicalAssetRecordV1 {
            schema_version: 1,
            registry_index: 0,
            asset_id: [0u8; 32],
            ethereum_token: [0x21; 20],
            token_owner_l2: field_to_bytes(StepField::from(201u64)),
            token_id_l2: field_to_bytes(StepField::from(202u64)),
            decimals: 9,
            inventory_cap: 1_000_000,
            mft_standard_vk_id: field_to_bytes(StepField::from(203u64)),
            vault_public_key: field_to_bytes(StepField::from(204u64)),
            universal_bridge_vk_id: field_to_bytes(StepField::from(205u64)),
        };
        first.asset_id = compute_asset_id(&first, chain_id, bridge_address);
        let first_leaf = hash_registry_record_leaf(&first);
        let first_root = field_to_bytes(registry_implied_root(first_leaf, 0, &first_path));

        let second_path = core::iter::once(field_to_bytes(first_leaf))
            .chain(
                zero_hashes[1..ASSET_REGISTRY_TREE_DEPTH]
                    .iter()
                    .copied()
                    .map(field_to_bytes),
            )
            .collect::<Vec<_>>();
        let mut second = CanonicalAssetRecordV1 {
            schema_version: 1,
            registry_index: 1,
            asset_id: [0u8; 32],
            ethereum_token: [0x22; 20],
            token_owner_l2: field_to_bytes(StepField::from(211u64)),
            token_id_l2: field_to_bytes(StepField::from(212u64)),
            decimals: 9,
            inventory_cap: 2_000_000,
            mft_standard_vk_id: first.mft_standard_vk_id,
            vault_public_key: first.vault_public_key,
            universal_bridge_vk_id: first.universal_bridge_vk_id,
        };
        second.asset_id = compute_asset_id(&second, chain_id, bridge_address);
        let second_root = field_to_bytes(registry_implied_root(
            hash_registry_record_leaf(&second),
            1,
            &second_path,
        ));
        let first_hash = hash_canonical_asset_record(&first);
        let second_hash = hash_canonical_asset_record(&second);
        let first_commitment = field_to_bytes(hash_registry_record_leaf(&first));
        let second_commitment = field_to_bytes(hash_registry_record_leaf(&second));
        let checkpoint = AssetRegistryBatchCheckpointV4 {
            registry_public_key: field_to_bytes(StepField::from(206u64)),
            root: second_root,
            count: 2,
            schema_version: 1,
            old_root: field_to_bytes(zero_hashes[ASSET_REGISTRY_TREE_DEPTH]),
            old_count: 0,
            appends: vec![
                AssetRegistryAppendV1 {
                    record: first,
                    append_path: first_path,
                },
                AssetRegistryAppendV1 {
                    record: second,
                    append_path: second_path,
                },
            ],
        };

        let identities = validate_registry_batch(&checkpoint, chain_id, bridge_address);
        assert_eq!(
            identities,
            vec![
                (0, first_hash, first_commitment),
                (1, second_hash, second_commitment)
            ]
        );
        assert_ne!(asset_record_batch_root(&identities), [0u8; 32]);
        assert_ne!(first_root, checkpoint.old_root);

        let mut swapped = checkpoint;
        swapped.appends.swap(0, 1);
        assert!(std::panic::catch_unwind(|| {
            validate_registry_batch(&swapped, chain_id, bridge_address)
        })
        .is_err());
    }
}
