use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize};
use std::fmt;

// SP1 guest execution is single-threaded, but some Mina/proof-system dependencies
// still reference GCC-style atomic symbols when compiled for riscv64im.
#[cfg(target_os = "zkvm")]
mod atomic_shims {
    use core::ptr::{read_volatile, write_volatile};

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_load_1(ptr: *const u8, _order: i32) -> u8 {
        read_volatile(ptr)
    }

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_load_8(ptr: *const u64, _order: i32) -> u64 {
        read_volatile(ptr)
    }

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_store_1(ptr: *mut u8, value: u8, _order: i32) {
        write_volatile(ptr, value);
    }

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_store_8(ptr: *mut u64, value: u64, _order: i32) {
        write_volatile(ptr, value);
    }

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_exchange_8(ptr: *mut u64, value: u64, _order: i32) -> u64 {
        let current = read_volatile(ptr);
        write_volatile(ptr, value);
        current
    }

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_compare_exchange_1(
        ptr: *mut u8,
        expected: *mut u8,
        desired: u8,
        _weak: bool,
        _success: i32,
        _failure: i32,
    ) -> bool {
        let current = read_volatile(ptr);
        if current == read_volatile(expected) {
            write_volatile(ptr, desired);
            true
        } else {
            write_volatile(expected, current);
            false
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_compare_exchange_8(
        ptr: *mut u64,
        expected: *mut u64,
        desired: u64,
        _weak: bool,
        _success: i32,
        _failure: i32,
    ) -> bool {
        let current = read_volatile(ptr);
        if current == read_volatile(expected) {
            write_volatile(ptr, desired);
            true
        } else {
            write_volatile(expected, current);
            false
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_fetch_add_8(ptr: *mut u64, value: u64, _order: i32) -> u64 {
        let current = read_volatile(ptr);
        write_volatile(ptr, current.wrapping_add(value));
        current
    }

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_fetch_sub_8(ptr: *mut u64, value: u64, _order: i32) -> u64 {
        let current = read_volatile(ptr);
        write_volatile(ptr, current.wrapping_sub(value));
        current
    }

    #[no_mangle]
    pub unsafe extern "C" fn __atomic_fetch_or_8(ptr: *mut u64, value: u64, _order: i32) -> u64 {
        let current = read_volatile(ptr);
        write_volatile(ptr, current | value);
        current
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ZkappPublicValues {
    pub proof_valid: bool,
    /// SHA256 vk hash
    pub vk_hash: [u8; 32],
    pub state_before: [[u8; 32]; 8],
    pub state_after: [[u8; 32]; 8],
    pub action_state_before: [u8; 32],
}

pub type Bytes32 = [u8; 32];
pub type Address = [u8; 20];
pub type ZekoAddress = Bytes32;

pub const SETTLEMENT_PUBLIC_VALUES_MAGIC: [u8; 4] = *b"ZKST";
pub const SETTLEMENT_PUBLIC_VALUES_VERSION: u16 = 1;
pub const SETTLEMENT_PUBLIC_VALUES_V2_VERSION: u16 = 2;
pub const SETTLEMENT_PUBLIC_VALUES_V3_VERSION: u16 = 3;
pub const SETTLEMENT_PUBLIC_VALUES_V4_VERSION: u16 = 4;
pub const SETTLEMENT_PUBLIC_VALUES_V1_LENGTH: usize = 768;
pub const SETTLEMENT_PUBLIC_VALUES_V2_LENGTH: usize = 828;
pub const SETTLEMENT_PUBLIC_VALUES_V3_LENGTH: usize = 932;
pub const SETTLEMENT_PUBLIC_VALUES_V4_LENGTH: usize = 904;
pub const ERC20_ACTION_ENCODING_V1: u32 = 1;
pub const ERC20_ACTION_ENCODING_V2: u32 = 2;

pub mod inner_action_commitment {
    use super::{
        Address, Bytes32, NativeWithdrawalV2, TokenWithdrawalV3, ERC20_ACTION_ENCODING_V1,
        ERC20_ACTION_ENCODING_V2,
    };
    use alloy_primitives::keccak256;

    pub const TREE_DEPTH: usize = 16;
    pub const MAX_LEAVES: usize = 1 << TREE_DEPTH;

    const ACTION_FIELDS_DOMAIN: &str = "ZEKO_INNER_ACTION_FIELDS_V2";
    const NATIVE_WITHDRAWAL_LEAF_DOMAIN: &str = "ZEKO_NATIVE_WITHDRAWAL_LEAF_V2";
    const ERC20_WITHDRAWAL_LEAF_V1_DOMAIN: &str = "ZEKO_ERC20_WITHDRAWAL_LEAF_V3";
    const ERC20_WITHDRAWAL_LEAF_V2_DOMAIN: &str = "ZEKO_ERC20_WITHDRAWAL_LEAF_V4";
    const RAW_INNER_ACTION_LEAF_DOMAIN: &str = "ZEKO_RAW_INNER_ACTION_LEAF_V2";
    const INNER_ACTION_NODE_DOMAIN: &str = "ZEKO_INNER_ACTION_NODE_V2";

    pub fn action_fields_hash(fields: &[Bytes32]) -> Bytes32 {
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

    pub fn native_withdrawal_leaf(
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

    pub fn erc20_withdrawal_leaf(
        chain_id: u64,
        bridge_address: Address,
        global_index: u32,
        withdrawal: &TokenWithdrawalV3,
        action_fields_hash: Bytes32,
    ) -> Bytes32 {
        let mut encoded = Vec::with_capacity(32 * 12);
        let domain = match withdrawal.encoding_version {
            ERC20_ACTION_ENCODING_V1 => ERC20_WITHDRAWAL_LEAF_V1_DOMAIN,
            ERC20_ACTION_ENCODING_V2 => ERC20_WITHDRAWAL_LEAF_V2_DOMAIN,
            version => panic!("unsupported ERC20 withdrawal encoding version {version}"),
        };
        encoded.extend_from_slice(&keccak256(domain.as_bytes()).0);
        encoded.extend_from_slice(&u64_word(chain_id));
        encoded.extend_from_slice(&address_word(bridge_address));
        encoded.extend_from_slice(&u32_word(global_index));
        encoded.extend_from_slice(&address_word(withdrawal.token));
        if withdrawal.encoding_version == ERC20_ACTION_ENCODING_V2 {
            encoded.extend_from_slice(&u32_word(withdrawal.encoding_version));
            encoded.extend_from_slice(&u32_word(withdrawal.registry_index));
            encoded.extend_from_slice(&withdrawal.record_commitment);
        }
        encoded.extend_from_slice(&withdrawal.asset_id);
        encoded.extend_from_slice(&address_word(withdrawal.recipient));
        encoded.extend_from_slice(&u64_word(withdrawal.amount));
        encoded.extend_from_slice(&action_fields_hash);
        keccak256(encoded).0
    }

    pub fn raw_inner_action_leaf(
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

    pub fn root(leaves: &[Bytes32]) -> Bytes32 {
        assert!(
            leaves.len() <= MAX_LEAVES,
            "inner action tree exceeds capacity"
        );
        let zero_hashes = zero_hashes();
        if leaves.is_empty() {
            return zero_hashes[TREE_DEPTH];
        }
        let mut nodes = leaves.to_vec();
        for level in 0..TREE_DEPTH {
            nodes = nodes
                .chunks(2)
                .map(|pair| hash_node(pair[0], pair.get(1).copied().unwrap_or(zero_hashes[level])))
                .collect();
        }
        assert_eq!(nodes.len(), 1, "invalid inner action tree");
        nodes[0]
    }

    pub fn merkle_proof(leaves: &[Bytes32], target: usize) -> Option<[Bytes32; TREE_DEPTH]> {
        if leaves.len() > MAX_LEAVES || target >= leaves.len() {
            return None;
        }
        let zero_hashes = zero_hashes();
        let mut proof = [[0u8; 32]; TREE_DEPTH];
        let mut nodes = leaves.to_vec();
        let mut index = target;
        for level in 0..TREE_DEPTH {
            proof[level] = nodes.get(index ^ 1).copied().unwrap_or(zero_hashes[level]);
            nodes = nodes
                .chunks(2)
                .map(|pair| hash_node(pair[0], pair.get(1).copied().unwrap_or(zero_hashes[level])))
                .collect();
            index >>= 1;
        }
        Some(proof)
    }

    pub fn verify_merkle_proof(
        leaf: Bytes32,
        mut index: usize,
        proof: &[Bytes32; TREE_DEPTH],
        expected_root: Bytes32,
    ) -> bool {
        if index >= MAX_LEAVES {
            return false;
        }
        let mut computed = leaf;
        for sibling in proof {
            computed = if index & 1 == 0 {
                hash_node(computed, *sibling)
            } else {
                hash_node(*sibling, computed)
            };
            index >>= 1;
        }
        computed == expected_root
    }

    fn zero_hashes() -> [Bytes32; TREE_DEPTH + 1] {
        let mut hashes = [[0u8; 32]; TREE_DEPTH + 1];
        for level in 0..TREE_DEPTH {
            hashes[level + 1] = hash_node(hashes[level], hashes[level]);
        }
        hashes
    }

    fn hash_node(left: Bytes32, right: Bytes32) -> Bytes32 {
        let mut encoded = Vec::with_capacity(96);
        encoded.extend_from_slice(&keccak256(INNER_ACTION_NODE_DOMAIN.as_bytes()).0);
        encoded.extend_from_slice(&left);
        encoded.extend_from_slice(&right);
        keccak256(encoded).0
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
}

/// Mina network domain used when hashing the account-update body that is the
/// first field of the verified Zkapp statement.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MinaSignatureKindV1 {
    Mainnet,
    Testnet,
}

/// A single chunk in Mina's `Random_oracle_input.Chunked` representation.
/// `value` must fit in `bits`; the guest checks this before packing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackedFieldV1 {
    #[serde(with = "serde_bytes32")]
    pub value: Bytes32,
    pub bits: u16,
}

/// Canonical preimage of `Account_update.Body.digest`, exported by OCaml.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChunkedRandomOracleInputV1 {
    #[serde(with = "serde_vec_bytes32")]
    pub field_elements: Vec<Bytes32>,
    pub packed: Vec<PackedFieldV1>,
}

/// Data that is already committed by the Pickles application statement. The
/// guest hashes `account_update_body`, compares it to the verified statement,
/// hashes `actions`, and decodes the fixed Zeko outer-commit action.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettlementBindingV1 {
    pub mina_signature_kind: MinaSignatureKindV1,
    pub account_update_body: ChunkedRandomOracleInputV1,
    #[serde(with = "serde_vec_vec_bytes32")]
    pub actions: Vec<Vec<Bytes32>>,
    pub state_before: OuterStateV1,
    /// Exact child call forest from the Pickles statement. It is required for
    /// V3 registry checkpoints and omitted by retained V1/V2 fixtures.
    #[serde(default)]
    pub call_forest: Vec<CallForestNodeV3>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CallForestNodeV3 {
    pub account_update_body: ChunkedRandomOracleInputV1,
    #[serde(default)]
    pub calls: Vec<CallForestNodeV3>,
}

/// Ethereum-domain values supplied by the gateway. These are intentionally
/// outside the Mina statement; Solidity checks all of them against L1 state.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettlementContextV1 {
    pub chain_id: u64,
    #[serde(with = "serde_address")]
    pub settlement_contract: Address,
    pub batch_sequence: u64,
    #[serde(with = "serde_bytes32")]
    pub mina_transaction_hash: Bytes32,
    pub outer_action_state_length_before: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettlementWitnessV1 {
    pub binding: SettlementBindingV1,
    pub context: SettlementContextV1,
    /// When present, the guest also proves the ordered inner-action range and
    /// emits a V2 receipt containing its Keccak claim tree. Keeping this field
    /// optional preserves the existing V1 fixture and execute checkpoint.
    #[serde(default)]
    pub inner_action_batch: Option<InnerActionBatchWitnessV2>,
    /// Optional proof-synchronized asset-registry checkpoint. The settlement
    /// guest emits V3 only together with the V2 inner-action range.
    #[serde(default)]
    pub asset_registry_checkpoint: Option<AssetRegistryCheckpointV3>,
    /// Optional batched proof-synchronized asset-registry checkpoint. V4
    /// validates every sequential Poseidon append and emits a depth-8 Keccak
    /// commitment to the exact canonical record hashes so Solidity can
    /// activate any record with a fixed-size membership proof.
    #[serde(default)]
    pub asset_registry_batch: Option<AssetRegistryBatchCheckpointV4>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetRegistryCheckpointV3 {
    /// Mina field encoding of the configured registry account public-key x
    /// coordinate. The guest uses it to select the authenticated child call.
    #[serde(with = "serde_bytes32")]
    pub registry_public_key: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub root: Bytes32,
    pub count: u32,
    pub schema_version: u32,
    /// Keccak hash of the exact canonical Solidity/TypeScript V1 asset record
    /// whose append produced this checkpoint.
    #[serde(with = "serde_bytes32")]
    pub record_hash: Bytes32,
    pub record: CanonicalAssetRecordV1,
    #[serde(with = "serde_vec_bytes32")]
    pub append_path: Vec<Bytes32>,
    #[serde(with = "serde_bytes32")]
    pub old_root: Bytes32,
    pub old_count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetRegistryBatchCheckpointV4 {
    /// Mina field encoding of the configured registry account public-key x
    /// coordinate. The guest uses it to select the authenticated child call.
    #[serde(with = "serde_bytes32")]
    pub registry_public_key: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub root: Bytes32,
    pub count: u32,
    pub schema_version: u32,
    #[serde(with = "serde_bytes32")]
    pub old_root: Bytes32,
    pub old_count: u32,
    pub appends: Vec<AssetRegistryAppendV1>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetRegistryAppendV1 {
    pub record: CanonicalAssetRecordV1,
    #[serde(with = "serde_vec_bytes32")]
    pub append_path: Vec<Bytes32>,
}

/// Canonical V1 asset wire shared with Solidity, OCaml and TypeScript.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalAssetRecordV1 {
    pub schema_version: u32,
    pub registry_index: u32,
    #[serde(with = "serde_bytes32")]
    pub asset_id: Bytes32,
    #[serde(with = "serde_address")]
    pub ethereum_token: Address,
    #[serde(with = "serde_bytes32")]
    pub token_owner_l2: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub token_id_l2: Bytes32,
    pub decimals: u8,
    pub inventory_cap: u64,
    #[serde(with = "serde_bytes32")]
    pub mft_standard_vk_id: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub vault_public_key: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub universal_bridge_vk_id: Bytes32,
}

/// A clear native withdrawal whose preimage must match the OCaml action aux.
/// Amounts use Zeko's native 9-decimal unit. Ethereum addresses are encoded as
/// synthetic compressed Mina keys `(x = uint160(address), is_odd = false)`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeWithdrawalV2 {
    #[serde(with = "serde_address")]
    pub recipient: Address,
    pub amount: u64,
}

/// Clear ERC-20 withdrawal metadata plus the exact flattened OCaml parameter
/// fields used to recompute the proof-bound action auxiliary hash.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TokenWithdrawalV3 {
    /// Explicit action encoding. Retained one-token fixtures deserialize as V1;
    /// universal-registry withdrawals must use V2.
    #[serde(default = "default_erc20_action_encoding")]
    pub encoding_version: u32,
    #[serde(default)]
    pub registry_index: u32,
    /// Mina Poseidon commitment to the complete canonical registry record.
    #[serde(default, with = "serde_bytes32")]
    pub record_commitment: Bytes32,
    #[serde(with = "serde_address")]
    pub token: Address,
    #[serde(with = "serde_bytes32")]
    pub asset_id: Bytes32,
    #[serde(with = "serde_address")]
    pub recipient: Address,
    pub amount: u64,
    #[serde(with = "serde_vec_bytes32")]
    pub params_fields: Vec<Bytes32>,
}

/// One exact OCaml `Rollup_state.Inner_action` action. `fields` is the raw
/// three-field action emitted by Mina. Non-withdrawal actions intentionally
/// omit `withdrawal` and become non-claimable leaves in the same ordered tree.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InnerActionWitnessV2 {
    #[serde(with = "serde_vec_bytes32")]
    pub fields: Vec<Bytes32>,
    #[serde(default)]
    pub withdrawal: Option<NativeWithdrawalV2>,
    #[serde(default)]
    pub token_withdrawal: Option<TokenWithdrawalV3>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InnerActionBatchWitnessV2 {
    #[serde(with = "serde_address")]
    pub bridge_address: Address,
    pub actions: Vec<InnerActionWitnessV2>,
}

/// The exact eight-field OCaml `Rollup_state.Outer_state` app-state layout.
/// Fields are canonical 32-byte Mina field encodings supplied by the OCaml
/// exporter; consumers should use the named accessors rather than numeric
/// indices.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct OuterStateV1 {
    #[serde(with = "serde_array8_bytes32")]
    pub fields: [Bytes32; 8],
}

impl OuterStateV1 {
    pub fn pause_key(&self) -> &Bytes32 {
        &self.fields[0]
    }

    pub fn status_flags(&self) -> &Bytes32 {
        &self.fields[1]
    }

    pub fn ledger_hash(&self) -> &Bytes32 {
        &self.fields[2]
    }

    pub fn inner_action_state(&self) -> &Bytes32 {
        &self.fields[3]
    }

    pub fn inner_action_state_length(&self) -> &Bytes32 {
        &self.fields[4]
    }

    pub fn sequencer(&self) -> &Bytes32 {
        &self.fields[5]
    }

    pub fn da_key(&self) -> &Bytes32 {
        &self.fields[6]
    }

    pub fn account_set(&self) -> &Bytes32 {
        &self.fields[7]
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SettlementDaMode {
    Multisig = 1,
}

/// Versioned receipt emitted by the settlement guest and decoded by Solidity.
/// Numeric values use big-endian encoding in [`Self::encode`] so the byte
/// layout is unambiguous across Rust, OCaml, and Solidity.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SettlementPublicValuesV1 {
    pub da_mode: SettlementDaMode,
    pub chain_id: u64,
    pub settlement_contract: Address,
    pub batch_sequence: u64,
    pub vk_hash: Bytes32,
    pub app_statement: Bytes32,
    pub mina_transaction_hash: Bytes32,
    pub state_before: OuterStateV1,
    pub state_after: OuterStateV1,
    pub outer_action_state_before: Bytes32,
    pub outer_action_state_after: Bytes32,
    pub outer_action_state_length_before: u32,
    pub outer_action_state_length_after: u32,
    pub synchronized_outer_action_state: Bytes32,
    pub synchronized_outer_action_state_length: u32,
    pub slot_lower: u32,
    pub slot_upper: u32,
}

impl SettlementPublicValuesV1 {
    pub fn encode(&self) -> [u8; SETTLEMENT_PUBLIC_VALUES_V1_LENGTH] {
        let mut output = [0u8; SETTLEMENT_PUBLIC_VALUES_V1_LENGTH];
        let mut cursor = 0;

        write_bytes(&mut output, &mut cursor, &SETTLEMENT_PUBLIC_VALUES_MAGIC);
        write_bytes(
            &mut output,
            &mut cursor,
            &SETTLEMENT_PUBLIC_VALUES_VERSION.to_be_bytes(),
        );
        write_bytes(&mut output, &mut cursor, &[self.da_mode as u8, 0]);
        write_bytes(&mut output, &mut cursor, &self.chain_id.to_be_bytes());
        write_bytes(&mut output, &mut cursor, &self.settlement_contract);
        write_bytes(&mut output, &mut cursor, &self.batch_sequence.to_be_bytes());
        write_bytes(&mut output, &mut cursor, &self.vk_hash);
        write_bytes(&mut output, &mut cursor, &self.app_statement);
        write_bytes(&mut output, &mut cursor, &self.mina_transaction_hash);
        for field in &self.state_before.fields {
            write_bytes(&mut output, &mut cursor, field);
        }
        for field in &self.state_after.fields {
            write_bytes(&mut output, &mut cursor, field);
        }
        write_bytes(&mut output, &mut cursor, &self.outer_action_state_before);
        write_bytes(&mut output, &mut cursor, &self.outer_action_state_after);
        write_bytes(
            &mut output,
            &mut cursor,
            &self.outer_action_state_length_before.to_be_bytes(),
        );
        write_bytes(
            &mut output,
            &mut cursor,
            &self.outer_action_state_length_after.to_be_bytes(),
        );
        write_bytes(
            &mut output,
            &mut cursor,
            &self.synchronized_outer_action_state,
        );
        write_bytes(
            &mut output,
            &mut cursor,
            &self.synchronized_outer_action_state_length.to_be_bytes(),
        );
        write_bytes(&mut output, &mut cursor, &self.slot_lower.to_be_bytes());
        write_bytes(&mut output, &mut cursor, &self.slot_upper.to_be_bytes());
        debug_assert_eq!(cursor, SETTLEMENT_PUBLIC_VALUES_V1_LENGTH);
        output
    }

    pub fn decode(input: &[u8]) -> Result<Self, String> {
        if input.len() != SETTLEMENT_PUBLIC_VALUES_V1_LENGTH {
            return Err(format!(
                "settlement public values: expected {} bytes, got {}",
                SETTLEMENT_PUBLIC_VALUES_V1_LENGTH,
                input.len()
            ));
        }
        let mut cursor = 0;
        let magic: [u8; 4] = read_array(input, &mut cursor);
        if magic != SETTLEMENT_PUBLIC_VALUES_MAGIC {
            return Err("settlement public values: invalid magic".to_owned());
        }
        let version = u16::from_be_bytes(read_array(input, &mut cursor));
        if version != SETTLEMENT_PUBLIC_VALUES_VERSION {
            return Err(format!(
                "settlement public values: unsupported version {version}"
            ));
        }
        let mode = read_array::<1>(input, &mut cursor)[0];
        let reserved = read_array::<1>(input, &mut cursor)[0];
        if reserved != 0 {
            return Err("settlement public values: reserved byte must be zero".to_owned());
        }
        let da_mode = match mode {
            1 => SettlementDaMode::Multisig,
            other => {
                return Err(format!(
                    "settlement public values: unsupported DA mode {other}"
                ))
            }
        };
        let chain_id = u64::from_be_bytes(read_array(input, &mut cursor));
        let settlement_contract = read_array(input, &mut cursor);
        let batch_sequence = u64::from_be_bytes(read_array(input, &mut cursor));
        let vk_hash = read_array(input, &mut cursor);
        let app_statement = read_array(input, &mut cursor);
        let mina_transaction_hash = read_array(input, &mut cursor);
        let mut state_before = OuterStateV1::default();
        for field in &mut state_before.fields {
            *field = read_array(input, &mut cursor);
        }
        let mut state_after = OuterStateV1::default();
        for field in &mut state_after.fields {
            *field = read_array(input, &mut cursor);
        }
        let outer_action_state_before = read_array(input, &mut cursor);
        let outer_action_state_after = read_array(input, &mut cursor);
        let outer_action_state_length_before = u32::from_be_bytes(read_array(input, &mut cursor));
        let outer_action_state_length_after = u32::from_be_bytes(read_array(input, &mut cursor));
        let synchronized_outer_action_state = read_array(input, &mut cursor);
        let synchronized_outer_action_state_length =
            u32::from_be_bytes(read_array(input, &mut cursor));
        let slot_lower = u32::from_be_bytes(read_array(input, &mut cursor));
        let slot_upper = u32::from_be_bytes(read_array(input, &mut cursor));
        debug_assert_eq!(cursor, input.len());

        Ok(Self {
            da_mode,
            chain_id,
            settlement_contract,
            batch_sequence,
            vk_hash,
            app_statement,
            mina_transaction_hash,
            state_before,
            state_after,
            outer_action_state_before,
            outer_action_state_after,
            outer_action_state_length_before,
            outer_action_state_length_after,
            synchronized_outer_action_state,
            synchronized_outer_action_state_length,
            slot_lower,
            slot_upper,
        })
    }
}

/// V2 extends the stable V1 prefix. The prefix has version 2 on the wire; all
/// appended integers are big-endian, matching the V1 Solidity decoder.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SettlementPublicValuesV2 {
    pub settlement: SettlementPublicValuesV1,
    pub bridge_address: Address,
    pub inner_action_root: Bytes32,
    pub inner_action_start_index: u32,
    pub inner_action_count: u32,
}

/// V3 extends the V2 receipt with the settled L2 asset-registry checkpoint and
/// exact canonical record identity used by the pending-to-active L1 flow.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SettlementPublicValuesV3 {
    pub settlement: SettlementPublicValuesV2,
    pub asset_registry_root: Bytes32,
    pub asset_registry_count: u32,
    pub asset_registry_schema_version: u32,
    pub asset_record_hash: Bytes32,
    /// Mina Poseidon commitment to the same record authenticated by
    /// `asset_record_hash`.
    pub asset_record_commitment: Bytes32,
}

/// V4 extends the V2 receipt with a settled L2 asset-registry checkpoint and a
/// depth-8 Keccak commitment to all exact record hashes appended by this
/// settlement. The tree uses global registry indices, so a membership proof
/// also binds each record to its canonical append position.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SettlementPublicValuesV4 {
    pub settlement: SettlementPublicValuesV2,
    pub asset_registry_root: Bytes32,
    pub asset_registry_count: u32,
    pub asset_registry_schema_version: u32,
    pub asset_record_batch_root: Bytes32,
    pub asset_record_batch_count: u32,
}

impl SettlementPublicValuesV4 {
    pub fn encode(&self) -> [u8; SETTLEMENT_PUBLIC_VALUES_V4_LENGTH] {
        let mut output = [0u8; SETTLEMENT_PUBLIC_VALUES_V4_LENGTH];
        output[..SETTLEMENT_PUBLIC_VALUES_V2_LENGTH].copy_from_slice(&self.settlement.encode());
        output[4..6].copy_from_slice(&SETTLEMENT_PUBLIC_VALUES_V4_VERSION.to_be_bytes());

        let mut cursor = SETTLEMENT_PUBLIC_VALUES_V2_LENGTH;
        write_bytes(&mut output, &mut cursor, &self.asset_registry_root);
        write_bytes(
            &mut output,
            &mut cursor,
            &self.asset_registry_count.to_be_bytes(),
        );
        write_bytes(
            &mut output,
            &mut cursor,
            &self.asset_registry_schema_version.to_be_bytes(),
        );
        write_bytes(&mut output, &mut cursor, &self.asset_record_batch_root);
        write_bytes(
            &mut output,
            &mut cursor,
            &self.asset_record_batch_count.to_be_bytes(),
        );
        debug_assert_eq!(cursor, SETTLEMENT_PUBLIC_VALUES_V4_LENGTH);
        output
    }

    pub fn decode(input: &[u8]) -> Result<Self, String> {
        if input.len() != SETTLEMENT_PUBLIC_VALUES_V4_LENGTH {
            return Err(format!(
                "settlement V4 public values: expected {} bytes, got {}",
                SETTLEMENT_PUBLIC_VALUES_V4_LENGTH,
                input.len()
            ));
        }
        if input[..4] != SETTLEMENT_PUBLIC_VALUES_MAGIC {
            return Err("settlement V4 public values: invalid magic".to_owned());
        }
        let version = u16::from_be_bytes(input[4..6].try_into().expect("two-byte version"));
        if version != SETTLEMENT_PUBLIC_VALUES_V4_VERSION {
            return Err(format!(
                "settlement V4 public values: unsupported version {version}"
            ));
        }

        let mut v2_prefix = [0u8; SETTLEMENT_PUBLIC_VALUES_V2_LENGTH];
        v2_prefix.copy_from_slice(&input[..SETTLEMENT_PUBLIC_VALUES_V2_LENGTH]);
        v2_prefix[4..6].copy_from_slice(&SETTLEMENT_PUBLIC_VALUES_V2_VERSION.to_be_bytes());
        let settlement = SettlementPublicValuesV2::decode(&v2_prefix)?;

        let mut cursor = SETTLEMENT_PUBLIC_VALUES_V2_LENGTH;
        let asset_registry_root = read_array(input, &mut cursor);
        let asset_registry_count = u32::from_be_bytes(read_array(input, &mut cursor));
        let asset_registry_schema_version = u32::from_be_bytes(read_array(input, &mut cursor));
        let asset_record_batch_root = read_array(input, &mut cursor);
        let asset_record_batch_count = u32::from_be_bytes(read_array(input, &mut cursor));
        debug_assert_eq!(cursor, input.len());

        Ok(Self {
            settlement,
            asset_registry_root,
            asset_registry_count,
            asset_registry_schema_version,
            asset_record_batch_root,
            asset_record_batch_count,
        })
    }
}

impl SettlementPublicValuesV3 {
    pub fn encode(&self) -> [u8; SETTLEMENT_PUBLIC_VALUES_V3_LENGTH] {
        let mut output = [0u8; SETTLEMENT_PUBLIC_VALUES_V3_LENGTH];
        output[..SETTLEMENT_PUBLIC_VALUES_V2_LENGTH].copy_from_slice(&self.settlement.encode());
        output[4..6].copy_from_slice(&SETTLEMENT_PUBLIC_VALUES_V3_VERSION.to_be_bytes());

        let mut cursor = SETTLEMENT_PUBLIC_VALUES_V2_LENGTH;
        write_bytes(&mut output, &mut cursor, &self.asset_registry_root);
        write_bytes(
            &mut output,
            &mut cursor,
            &self.asset_registry_count.to_be_bytes(),
        );
        write_bytes(
            &mut output,
            &mut cursor,
            &self.asset_registry_schema_version.to_be_bytes(),
        );
        write_bytes(&mut output, &mut cursor, &self.asset_record_hash);
        write_bytes(&mut output, &mut cursor, &self.asset_record_commitment);
        debug_assert_eq!(cursor, SETTLEMENT_PUBLIC_VALUES_V3_LENGTH);
        output
    }

    pub fn decode(input: &[u8]) -> Result<Self, String> {
        if input.len() != SETTLEMENT_PUBLIC_VALUES_V3_LENGTH {
            return Err(format!(
                "settlement V3 public values: expected {} bytes, got {}",
                SETTLEMENT_PUBLIC_VALUES_V3_LENGTH,
                input.len()
            ));
        }
        if input[..4] != SETTLEMENT_PUBLIC_VALUES_MAGIC {
            return Err("settlement V3 public values: invalid magic".to_owned());
        }
        let version = u16::from_be_bytes(input[4..6].try_into().expect("two-byte version"));
        if version != SETTLEMENT_PUBLIC_VALUES_V3_VERSION {
            return Err(format!(
                "settlement V3 public values: unsupported version {version}"
            ));
        }

        let mut v2_prefix = [0u8; SETTLEMENT_PUBLIC_VALUES_V2_LENGTH];
        v2_prefix.copy_from_slice(&input[..SETTLEMENT_PUBLIC_VALUES_V2_LENGTH]);
        v2_prefix[4..6].copy_from_slice(&SETTLEMENT_PUBLIC_VALUES_V2_VERSION.to_be_bytes());
        let settlement = SettlementPublicValuesV2::decode(&v2_prefix)?;

        let mut cursor = SETTLEMENT_PUBLIC_VALUES_V2_LENGTH;
        let asset_registry_root = read_array(input, &mut cursor);
        let asset_registry_count = u32::from_be_bytes(read_array(input, &mut cursor));
        let asset_registry_schema_version = u32::from_be_bytes(read_array(input, &mut cursor));
        let asset_record_hash = read_array(input, &mut cursor);
        let asset_record_commitment = read_array(input, &mut cursor);
        debug_assert_eq!(cursor, input.len());

        Ok(Self {
            settlement,
            asset_registry_root,
            asset_registry_count,
            asset_registry_schema_version,
            asset_record_hash,
            asset_record_commitment,
        })
    }
}

impl SettlementPublicValuesV2 {
    pub fn encode(&self) -> [u8; SETTLEMENT_PUBLIC_VALUES_V2_LENGTH] {
        let mut output = [0u8; SETTLEMENT_PUBLIC_VALUES_V2_LENGTH];
        output[..SETTLEMENT_PUBLIC_VALUES_V1_LENGTH].copy_from_slice(&self.settlement.encode());
        output[4..6].copy_from_slice(&SETTLEMENT_PUBLIC_VALUES_V2_VERSION.to_be_bytes());

        let mut cursor = SETTLEMENT_PUBLIC_VALUES_V1_LENGTH;
        write_bytes(&mut output, &mut cursor, &self.bridge_address);
        write_bytes(&mut output, &mut cursor, &self.inner_action_root);
        write_bytes(
            &mut output,
            &mut cursor,
            &self.inner_action_start_index.to_be_bytes(),
        );
        write_bytes(
            &mut output,
            &mut cursor,
            &self.inner_action_count.to_be_bytes(),
        );
        debug_assert_eq!(cursor, SETTLEMENT_PUBLIC_VALUES_V2_LENGTH);
        output
    }

    pub fn decode(input: &[u8]) -> Result<Self, String> {
        if input.len() != SETTLEMENT_PUBLIC_VALUES_V2_LENGTH {
            return Err(format!(
                "settlement V2 public values: expected {} bytes, got {}",
                SETTLEMENT_PUBLIC_VALUES_V2_LENGTH,
                input.len()
            ));
        }
        if input[..4] != SETTLEMENT_PUBLIC_VALUES_MAGIC {
            return Err("settlement V2 public values: invalid magic".to_owned());
        }
        let version = u16::from_be_bytes(input[4..6].try_into().expect("two-byte version"));
        if version != SETTLEMENT_PUBLIC_VALUES_V2_VERSION {
            return Err(format!(
                "settlement V2 public values: unsupported version {version}"
            ));
        }

        let mut v1_prefix = [0u8; SETTLEMENT_PUBLIC_VALUES_V1_LENGTH];
        v1_prefix.copy_from_slice(&input[..SETTLEMENT_PUBLIC_VALUES_V1_LENGTH]);
        v1_prefix[4..6].copy_from_slice(&SETTLEMENT_PUBLIC_VALUES_VERSION.to_be_bytes());
        let settlement = SettlementPublicValuesV1::decode(&v1_prefix)?;

        let mut cursor = SETTLEMENT_PUBLIC_VALUES_V1_LENGTH;
        let bridge_address = read_array(input, &mut cursor);
        let inner_action_root = read_array(input, &mut cursor);
        let inner_action_start_index = u32::from_be_bytes(read_array(input, &mut cursor));
        let inner_action_count = u32::from_be_bytes(read_array(input, &mut cursor));
        debug_assert_eq!(cursor, input.len());

        Ok(Self {
            settlement,
            bridge_address,
            inner_action_root,
            inner_action_start_index,
            inner_action_count,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettlementPublicValues {
    V1(SettlementPublicValuesV1),
    V2(SettlementPublicValuesV2),
    V3(SettlementPublicValuesV3),
    V4(SettlementPublicValuesV4),
}

impl SettlementPublicValues {
    pub fn decode(input: &[u8]) -> Result<Self, String> {
        match input.len() {
            SETTLEMENT_PUBLIC_VALUES_V1_LENGTH => {
                SettlementPublicValuesV1::decode(input).map(Self::V1)
            }
            SETTLEMENT_PUBLIC_VALUES_V2_LENGTH => {
                SettlementPublicValuesV2::decode(input).map(Self::V2)
            }
            SETTLEMENT_PUBLIC_VALUES_V3_LENGTH => {
                SettlementPublicValuesV3::decode(input).map(Self::V3)
            }
            SETTLEMENT_PUBLIC_VALUES_V4_LENGTH => {
                SettlementPublicValuesV4::decode(input).map(Self::V4)
            }
            actual => Err(format!(
                "settlement public values: unsupported length {actual}"
            )),
        }
    }

    pub fn settlement(&self) -> &SettlementPublicValuesV1 {
        match self {
            Self::V1(values) => values,
            Self::V2(values) => &values.settlement,
            Self::V3(values) => &values.settlement.settlement,
            Self::V4(values) => &values.settlement.settlement,
        }
    }

    pub fn inner_action_batch(&self) -> Option<&SettlementPublicValuesV2> {
        match self {
            Self::V1(_) => None,
            Self::V2(values) => Some(values),
            Self::V3(values) => Some(&values.settlement),
            Self::V4(values) => Some(&values.settlement),
        }
    }
}

fn write_bytes<const N: usize>(output: &mut [u8], cursor: &mut usize, bytes: &[u8; N]) {
    output[*cursor..*cursor + N].copy_from_slice(bytes);
    *cursor += N;
}

fn read_array<const N: usize>(input: &[u8], cursor: &mut usize) -> [u8; N] {
    let result = input[*cursor..*cursor + N]
        .try_into()
        .expect("public values length checked");
    *cursor += N;
    result
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeDeposit {
    /// Zero for native ETH; otherwise the registered ERC-20 address.
    #[serde(default, with = "serde_address")]
    pub token: Address,
    /// Canonical registry identity. Native ETH uses zero for compatibility.
    #[serde(default, with = "serde_bytes32")]
    pub asset_id: Bytes32,
    /// Explicit action encoding. Retained one-token fixtures deserialize as V1;
    /// universal-registry deposits must use V2.
    #[serde(default = "default_erc20_action_encoding")]
    pub encoding_version: u32,
    #[serde(default)]
    pub registry_index: u32,
    /// Mina Poseidon commitment to the complete canonical registry record.
    #[serde(default, with = "serde_bytes32")]
    pub record_commitment: Bytes32,
    /// Raw Ethereum custody amount. Native ETH uses wei; canonical ERC-20s use
    /// the identical base unit configured on the Mina fungible token.
    #[serde(with = "serde_bytes32")]
    pub amount: Bytes32,
    /// Amount encoded into the Zeko action. Older native fixtures omit this and
    /// let the guest derive the 9-decimal amount from wei.
    #[serde(default)]
    pub zeko_amount: Option<u64>,
    #[serde(with = "serde_bytes32")]
    pub zeko_recipient: ZekoAddress,
    #[serde(default = "default_bridge_timeout")]
    pub timeout: u64,
}

fn default_bridge_timeout() -> u64 {
    u32::MAX as u64
}

fn default_erc20_action_encoding() -> u32 {
    ERC20_ACTION_ENCODING_V1
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeWithdraw {
    #[serde(with = "serde_bytes32")]
    pub token: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub recipient: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub amount: Bytes32,
    /// Digest of the zkapp call forest attached to this withdrawal action (fields[2]).
    #[serde(with = "serde_bytes32")]
    pub children_digest: Bytes32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EthereumBridgeState {
    pub chain_id: u64,
    #[serde(with = "serde_address")]
    pub bridge_address: Address,
    pub deposit_nonce: u64,
    #[serde(with = "serde_bytes32")]
    pub deposit_state: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub withdraw_state: Bytes32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ZekoBridgeState {
    #[serde(with = "serde_bytes32")]
    pub action_state: Bytes32,
    #[serde(default)]
    pub action_state_length: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeTransitionInput {
    pub ethereum: EthereumBridgeState,
    pub zeko: ZekoBridgeState,
    pub deposits: Vec<BridgeDeposit>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeTransitionPublicValues {
    #[serde(with = "serde_bytes32")]
    pub ethereum_state_before: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub ethereum_state_after: Bytes32,
    pub ethereum_nonce_before: u64,
    pub ethereum_nonce_after: u64,
    #[serde(with = "serde_bytes32")]
    pub zeko_action_state_before: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub zeko_action_state_after: Bytes32,
    pub deposit_count: u32,
}

pub const BRIDGE_PUBLIC_VALUES_V2_MAGIC: [u8; 4] = *b"ZKBR";
pub const BRIDGE_PUBLIC_VALUES_V2_VERSION: u16 = 2;
pub const BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH: usize = 164;
pub const BRIDGE_ACTION_FIELDS: usize = 5;
pub const BRIDGE_ACTION_V2_LENGTH: usize = (BRIDGE_ACTION_FIELDS + 1) * 32;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeOuterActionV2 {
    pub fields: [Bytes32; BRIDGE_ACTION_FIELDS],
    pub state_after: Bytes32,
}

/// Canonical native/ERC-20 deposit receipt. The variable tail contains every
/// exact five-field outer Witness action, so the gateway can serve the same
/// action bytes to the OCaml sequencer without reimplementing Poseidon hashing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeTransitionPublicValuesV2 {
    pub ethereum_state_before: Bytes32,
    pub ethereum_state_after: Bytes32,
    pub ethereum_nonce_before: u64,
    pub ethereum_nonce_after: u64,
    pub zeko_action_state_before: Bytes32,
    pub zeko_action_state_after: Bytes32,
    pub zeko_action_state_length_before: u32,
    pub zeko_action_state_length_after: u32,
    pub actions: Vec<BridgeOuterActionV2>,
}

impl BridgeTransitionPublicValuesV2 {
    pub fn encode(&self) -> Vec<u8> {
        let count = u32::try_from(self.actions.len()).expect("bridge action count fits u32");
        let mut output = Vec::with_capacity(
            BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH + self.actions.len() * BRIDGE_ACTION_V2_LENGTH,
        );
        output.extend_from_slice(&BRIDGE_PUBLIC_VALUES_V2_MAGIC);
        output.extend_from_slice(&BRIDGE_PUBLIC_VALUES_V2_VERSION.to_be_bytes());
        output.extend_from_slice(&[0u8; 2]);
        output.extend_from_slice(&self.ethereum_state_before);
        output.extend_from_slice(&self.ethereum_state_after);
        output.extend_from_slice(&self.ethereum_nonce_before.to_be_bytes());
        output.extend_from_slice(&self.ethereum_nonce_after.to_be_bytes());
        output.extend_from_slice(&self.zeko_action_state_before);
        output.extend_from_slice(&self.zeko_action_state_after);
        output.extend_from_slice(&self.zeko_action_state_length_before.to_be_bytes());
        output.extend_from_slice(&self.zeko_action_state_length_after.to_be_bytes());
        output.extend_from_slice(&count.to_be_bytes());
        for action in &self.actions {
            for field in &action.fields {
                output.extend_from_slice(field);
            }
            output.extend_from_slice(&action.state_after);
        }
        debug_assert_eq!(output.len(), output.capacity());
        output
    }

    pub fn decode(input: &[u8]) -> Result<Self, String> {
        if input.len() < BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH {
            return Err("bridge V2 public values: truncated header".to_owned());
        }
        if input[..4] != BRIDGE_PUBLIC_VALUES_V2_MAGIC {
            return Err("bridge V2 public values: invalid magic".to_owned());
        }
        let version = u16::from_be_bytes(input[4..6].try_into().expect("two-byte version"));
        if version != BRIDGE_PUBLIC_VALUES_V2_VERSION {
            return Err(format!(
                "bridge V2 public values: unsupported version {version}"
            ));
        }
        if input[6..8] != [0u8; 2] {
            return Err("bridge V2 public values: reserved bytes must be zero".to_owned());
        }
        let mut cursor = 8;
        let ethereum_state_before = read_array(input, &mut cursor);
        let ethereum_state_after = read_array(input, &mut cursor);
        let ethereum_nonce_before = u64::from_be_bytes(read_array(input, &mut cursor));
        let ethereum_nonce_after = u64::from_be_bytes(read_array(input, &mut cursor));
        let zeko_action_state_before = read_array(input, &mut cursor);
        let zeko_action_state_after = read_array(input, &mut cursor);
        let zeko_action_state_length_before = u32::from_be_bytes(read_array(input, &mut cursor));
        let zeko_action_state_length_after = u32::from_be_bytes(read_array(input, &mut cursor));
        let count = u32::from_be_bytes(read_array(input, &mut cursor)) as usize;
        let expected = BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH
            .checked_add(count * BRIDGE_ACTION_V2_LENGTH)
            .ok_or_else(|| "bridge V2 public values: length overflow".to_owned())?;
        if input.len() != expected {
            return Err(format!(
                "bridge V2 public values: expected {expected} bytes, got {}",
                input.len()
            ));
        }
        let mut actions = Vec::with_capacity(count);
        for _ in 0..count {
            actions.push(BridgeOuterActionV2 {
                fields: core::array::from_fn(|_| read_array(input, &mut cursor)),
                state_after: read_array(input, &mut cursor),
            });
        }
        Ok(Self {
            ethereum_state_before,
            ethereum_state_after,
            ethereum_nonce_before,
            ethereum_nonce_after,
            zeko_action_state_before,
            zeko_action_state_after,
            zeko_action_state_length_before,
            zeko_action_state_length_after,
            actions,
        })
    }

    pub fn deposit_count(&self) -> u32 {
        self.actions.len() as u32
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WithdrawTransitionInput {
    pub ethereum: EthereumBridgeState,
    pub zeko: ZekoBridgeState,
    pub withdraws: Vec<BridgeWithdraw>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WithdrawTransitionPublicValues {
    #[serde(with = "serde_bytes32")]
    pub zeko_action_state_before: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub zeko_action_state_after: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub ethereum_withdraw_state_before: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub ethereum_withdraw_state_after: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub withdrawal_root: Bytes32,
    pub withdraw_count: u32,
}

mod serde_address {
    use super::*;

    pub fn serialize<S>(value: &Address, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_fixed_bytes(value, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Address, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_fixed_bytes(deserializer)
    }
}

mod serde_bytes32 {
    use super::*;

    pub fn serialize<S>(value: &Bytes32, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_fixed_bytes(value, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Bytes32, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_fixed_bytes(deserializer)
    }
}

mod serde_vec_bytes32 {
    use super::*;

    pub fn serialize<S>(values: &[Bytes32], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut sequence = serializer.serialize_seq(Some(values.len()))?;
        for value in values {
            sequence.serialize_element(&fixed_bytes_to_hex(value))?;
        }
        sequence.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Bytes32>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let values = Vec::<String>::deserialize(deserializer)?;
        values
            .iter()
            .map(|value| parse_fixed_bytes(value).map_err(serde::de::Error::custom))
            .collect()
    }
}

mod serde_vec_vec_bytes32 {
    use super::*;

    pub fn serialize<S>(values: &[Vec<Bytes32>], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let encoded = values
            .iter()
            .map(|event| event.iter().map(fixed_bytes_to_hex).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        encoded.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Vec<Bytes32>>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let values = Vec::<Vec<String>>::deserialize(deserializer)?;
        values
            .iter()
            .map(|event| {
                event
                    .iter()
                    .map(|value| parse_fixed_bytes(value).map_err(serde::de::Error::custom))
                    .collect()
            })
            .collect()
    }
}

mod serde_array8_bytes32 {
    use super::*;

    pub fn serialize<S>(values: &[Bytes32; 8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if !serializer.is_human_readable() {
            return values.serialize(serializer);
        }
        let encoded = values.map(|value| fixed_bytes_to_hex(&value));
        encoded.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[Bytes32; 8], D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if !deserializer.is_human_readable() {
            return <[Bytes32; 8]>::deserialize(deserializer);
        }
        let values = Vec::<String>::deserialize(deserializer)?;
        if values.len() != 8 {
            return Err(serde::de::Error::invalid_length(
                values.len(),
                &"eight fields",
            ));
        }
        let decoded = values
            .iter()
            .map(|value| parse_fixed_bytes(value).map_err(serde::de::Error::custom))
            .collect::<Result<Vec<Bytes32>, D::Error>>()?;
        decoded
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected eight fields"))
    }
}

fn serialize_fixed_bytes<const N: usize, S>(
    value: &[u8; N],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if serializer.is_human_readable() {
        serializer.serialize_str(&fixed_bytes_to_hex(value))
    } else {
        let mut tuple = serializer.serialize_tuple(N)?;
        for byte in value {
            tuple.serialize_element(byte)?;
        }
        tuple.end()
    }
}

fn deserialize_fixed_bytes<'de, const N: usize, D>(deserializer: D) -> Result<[u8; N], D::Error>
where
    D: serde::Deserializer<'de>,
{
    if deserializer.is_human_readable() {
        deserializer.deserialize_any(FixedBytesVisitor::<N>)
    } else {
        deserializer.deserialize_tuple(N, FixedBytesVisitor::<N>)
    }
}

struct FixedBytesVisitor<const N: usize>;

impl<'de, const N: usize> serde::de::Visitor<'de> for FixedBytesVisitor<N> {
    type Value = [u8; N];

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "a 0x-prefixed hex string, decimal uint256 string, or {N}-byte array"
        )
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        parse_fixed_bytes(value).map_err(E::custom)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        self.visit_str(&value)
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        integer_to_fixed_bytes(value as u128)
    }

    fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        integer_to_fixed_bytes(value)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = seq
                .next_element::<u8>()?
                .ok_or_else(|| serde::de::Error::invalid_length(i, &self))?;
        }
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::invalid_length(N + 1, &self));
        }
        Ok(out)
    }
}

fn integer_to_fixed_bytes<const N: usize, E>(value: u128) -> Result<[u8; N], E>
where
    E: serde::de::Error,
{
    if N != 32 {
        return Err(E::custom(
            "JSON numbers are only supported for uint256/bytes32 fields",
        ));
    }

    let mut out = [0u8; N];
    out[N - 16..].copy_from_slice(&value.to_be_bytes());
    Ok(out)
}

fn fixed_bytes_to_hex<const N: usize>(value: &[u8; N]) -> String {
    let mut out = String::with_capacity(2 + N * 2);
    out.push_str("0x");
    for byte in value {
        use std::fmt::Write;
        write!(&mut out, "{byte:02x}").expect("write hex");
    }
    out
}

fn parse_fixed_bytes<const N: usize>(value: &str) -> Result<[u8; N], String> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        return parse_hex_fixed(hex);
    }

    if N != 32 {
        return Err("decimal strings are only supported for uint256/bytes32 fields".to_string());
    }

    parse_decimal_u256(trimmed).map(|bytes| {
        let mut out = [0u8; N];
        out.copy_from_slice(&bytes);
        out
    })
}

fn parse_hex_fixed<const N: usize>(hex: &str) -> Result<[u8; N], String> {
    if hex.len() > N * 2 {
        return Err(format!("hex string is too long for {N} bytes"));
    }
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("hex string contains a non-hex character".to_string());
    }

    let mut out = [0u8; N];
    let mut nibble_index = N * 2 - hex.len();
    for b in hex.bytes() {
        let nibble = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => unreachable!(),
        };
        let byte_index = nibble_index / 2;
        if nibble_index % 2 == 0 {
            out[byte_index] = nibble << 4;
        } else {
            out[byte_index] |= nibble;
        }
        nibble_index += 1;
    }
    Ok(out)
}

fn parse_decimal_u256(value: &str) -> Result<Bytes32, String> {
    if value.is_empty() {
        return Err("empty decimal string".to_string());
    }

    let mut out = [0u8; 32];
    for digit in value.bytes() {
        if !digit.is_ascii_digit() {
            return Err("decimal string contains a non-digit character".to_string());
        }

        let mut carry = (digit - b'0') as u16;
        for byte in out.iter_mut().rev() {
            let next = (*byte as u16) * 10 + carry;
            *byte = next as u8;
            carry = next >> 8;
        }
        if carry != 0 {
            return Err("decimal string overflows uint256".to_string());
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settlement_values() -> SettlementPublicValuesV1 {
        SettlementPublicValuesV1 {
            da_mode: SettlementDaMode::Multisig,
            chain_id: 11_155_111,
            settlement_contract: [0x11; 20],
            batch_sequence: 42,
            vk_hash: [0x22; 32],
            app_statement: [0x33; 32],
            mina_transaction_hash: [0x44; 32],
            state_before: OuterStateV1 {
                fields: core::array::from_fn(|i| [i as u8; 32]),
            },
            state_after: OuterStateV1 {
                fields: core::array::from_fn(|i| [0x80 + i as u8; 32]),
            },
            outer_action_state_before: [0x55; 32],
            outer_action_state_after: [0x66; 32],
            outer_action_state_length_before: 7,
            outer_action_state_length_after: 8,
            synchronized_outer_action_state: [0x77; 32],
            synchronized_outer_action_state_length: 6,
            slot_lower: 100,
            slot_upper: 120,
        }
    }

    #[test]
    fn settlement_public_values_v1_round_trip() {
        let values = settlement_values();
        let encoded = values.encode();

        assert_eq!(encoded.len(), SETTLEMENT_PUBLIC_VALUES_V1_LENGTH);
        assert_eq!(&encoded[..4], b"ZKST");
        assert_eq!(SettlementPublicValuesV1::decode(&encoded).unwrap(), values);
    }

    #[test]
    fn settlement_public_values_v1_rejects_domain_drift() {
        let mut encoded = settlement_values().encode();
        encoded[0] ^= 1;
        assert!(SettlementPublicValuesV1::decode(&encoded).is_err());

        let mut encoded = settlement_values().encode();
        encoded[4..6].copy_from_slice(&2u16.to_be_bytes());
        assert!(SettlementPublicValuesV1::decode(&encoded).is_err());

        let mut encoded = settlement_values().encode();
        encoded[7] = 1;
        assert!(SettlementPublicValuesV1::decode(&encoded).is_err());
    }

    #[test]
    fn settlement_public_values_v3_round_trip() {
        let values = SettlementPublicValuesV3 {
            settlement: SettlementPublicValuesV2 {
                settlement: settlement_values(),
                bridge_address: [0x88; 20],
                inner_action_root: [0x99; 32],
                inner_action_start_index: 7,
                inner_action_count: 1,
            },
            asset_registry_root: [0xaa; 32],
            asset_registry_count: 3,
            asset_registry_schema_version: 1,
            asset_record_hash: [0xbb; 32],
            asset_record_commitment: [0xcc; 32],
        };
        let encoded = values.encode();
        assert_eq!(encoded.len(), SETTLEMENT_PUBLIC_VALUES_V3_LENGTH);
        assert_eq!(SettlementPublicValuesV3::decode(&encoded).unwrap(), values);
        let decoded = SettlementPublicValues::decode(&encoded).unwrap();
        assert!(matches!(&decoded, SettlementPublicValues::V3(_)));
        assert_eq!(decoded.inner_action_batch(), Some(&values.settlement));
    }

    #[test]
    fn settlement_public_values_v4_round_trip() {
        let values = SettlementPublicValuesV4 {
            settlement: SettlementPublicValuesV2 {
                settlement: settlement_values(),
                bridge_address: [0x88; 20],
                inner_action_root: [0x99; 32],
                inner_action_start_index: 7,
                inner_action_count: 2,
            },
            asset_registry_root: [0xaa; 32],
            asset_registry_count: 5,
            asset_registry_schema_version: 1,
            asset_record_batch_root: [0xbb; 32],
            asset_record_batch_count: 2,
        };
        let encoded = values.encode();
        assert_eq!(encoded.len(), SETTLEMENT_PUBLIC_VALUES_V4_LENGTH);
        assert_eq!(SettlementPublicValuesV4::decode(&encoded).unwrap(), values);
        let decoded = SettlementPublicValues::decode(&encoded).unwrap();
        assert!(matches!(&decoded, SettlementPublicValues::V4(_)));
        assert_eq!(decoded.inner_action_batch(), Some(&values.settlement));
    }

    #[test]
    fn settlement_binding_uses_hex_json_and_round_trips() {
        let binding = SettlementBindingV1 {
            mina_signature_kind: MinaSignatureKindV1::Testnet,
            account_update_body: ChunkedRandomOracleInputV1 {
                field_elements: vec![[0x11; 32], [0x22; 32]],
                packed: vec![PackedFieldV1 {
                    value: [1; 32],
                    bits: 8,
                }],
            },
            actions: vec![vec![[0x33; 32], [0x44; 32]]],
            state_before: OuterStateV1 {
                fields: [[0x55; 32]; 8],
            },
            call_forest: Vec::new(),
        };
        let json = serde_json::to_string(&binding).unwrap();
        assert!(json.contains("0x1111111111111111"));
        assert_eq!(
            serde_json::from_str::<SettlementBindingV1>(&json).unwrap(),
            binding
        );

        let witness = SettlementWitnessV1 {
            binding,
            context: SettlementContextV1 {
                chain_id: 1,
                settlement_contract: [0x66; 20],
                batch_sequence: 2,
                mina_transaction_hash: [0x77; 32],
                outer_action_state_length_before: 3,
            },
            inner_action_batch: None,
            asset_registry_checkpoint: None,
            asset_registry_batch: None,
        };
        let encoded = bincode::serialize(&witness).unwrap();
        assert_eq!(
            bincode::deserialize::<SettlementWitnessV1>(&encoded).unwrap(),
            witness
        );
    }

    #[test]
    fn inner_action_commitments_match_versioned_reference_encodings() {
        fn u64_word(value: u64) -> Bytes32 {
            let mut word = [0u8; 32];
            word[24..].copy_from_slice(&value.to_be_bytes());
            word
        }

        fn u32_word(value: u32) -> Bytes32 {
            let mut word = [0u8; 32];
            word[28..].copy_from_slice(&value.to_be_bytes());
            word
        }

        fn address_word(value: Address) -> Bytes32 {
            let mut word = [0u8; 32];
            word[12..].copy_from_slice(&value);
            word
        }

        let chain_id = 31_337;
        let bridge = [0x11; 20];
        let global_index = 5;
        let fields = [[0x21; 32], [0x22; 32], [0x23; 32]];
        let action_fields_hash = inner_action_commitment::action_fields_hash(&fields);
        let mut expected_action_fields = Vec::new();
        expected_action_fields
            .extend_from_slice(&alloy_primitives::keccak256("ZEKO_INNER_ACTION_FIELDS_V2").0);
        expected_action_fields.extend_from_slice(&u32_word(3));
        for field in fields {
            expected_action_fields.extend_from_slice(&field);
        }
        assert_eq!(
            action_fields_hash,
            alloy_primitives::keccak256(expected_action_fields).0
        );

        let native = NativeWithdrawalV2 {
            recipient: [0x31; 20],
            amount: 7,
        };
        let mut expected_native = Vec::new();
        expected_native
            .extend_from_slice(&alloy_primitives::keccak256("ZEKO_NATIVE_WITHDRAWAL_LEAF_V2").0);
        expected_native.extend_from_slice(&u64_word(chain_id));
        expected_native.extend_from_slice(&address_word(bridge));
        expected_native.extend_from_slice(&u32_word(global_index));
        expected_native.extend_from_slice(&address_word(native.recipient));
        expected_native.extend_from_slice(&u64_word(native.amount));
        expected_native.extend_from_slice(&action_fields_hash);
        assert_eq!(
            inner_action_commitment::native_withdrawal_leaf(
                chain_id,
                bridge,
                global_index,
                &native,
                action_fields_hash,
            ),
            alloy_primitives::keccak256(expected_native).0
        );

        for encoding_version in [ERC20_ACTION_ENCODING_V1, ERC20_ACTION_ENCODING_V2] {
            let withdrawal = TokenWithdrawalV3 {
                encoding_version,
                registry_index: 9,
                record_commitment: [0x41; 32],
                token: [0x42; 20],
                asset_id: [0x43; 32],
                recipient: [0x44; 20],
                amount: 11,
                params_fields: Vec::new(),
            };
            let domain = if encoding_version == ERC20_ACTION_ENCODING_V1 {
                "ZEKO_ERC20_WITHDRAWAL_LEAF_V3"
            } else {
                "ZEKO_ERC20_WITHDRAWAL_LEAF_V4"
            };
            let mut expected = Vec::new();
            expected.extend_from_slice(&alloy_primitives::keccak256(domain).0);
            expected.extend_from_slice(&u64_word(chain_id));
            expected.extend_from_slice(&address_word(bridge));
            expected.extend_from_slice(&u32_word(global_index));
            expected.extend_from_slice(&address_word(withdrawal.token));
            if encoding_version == ERC20_ACTION_ENCODING_V2 {
                expected.extend_from_slice(&u32_word(encoding_version));
                expected.extend_from_slice(&u32_word(withdrawal.registry_index));
                expected.extend_from_slice(&withdrawal.record_commitment);
            }
            expected.extend_from_slice(&withdrawal.asset_id);
            expected.extend_from_slice(&address_word(withdrawal.recipient));
            expected.extend_from_slice(&u64_word(withdrawal.amount));
            expected.extend_from_slice(&action_fields_hash);
            assert_eq!(
                inner_action_commitment::erc20_withdrawal_leaf(
                    chain_id,
                    bridge,
                    global_index,
                    &withdrawal,
                    action_fields_hash,
                ),
                alloy_primitives::keccak256(expected).0
            );
        }

        let mut expected_raw = Vec::new();
        expected_raw
            .extend_from_slice(&alloy_primitives::keccak256("ZEKO_RAW_INNER_ACTION_LEAF_V2").0);
        expected_raw.extend_from_slice(&u64_word(chain_id));
        expected_raw.extend_from_slice(&address_word(bridge));
        expected_raw.extend_from_slice(&u32_word(global_index));
        expected_raw.extend_from_slice(&action_fields_hash);
        assert_eq!(
            inner_action_commitment::raw_inner_action_leaf(
                chain_id,
                bridge,
                global_index,
                action_fields_hash,
            ),
            alloy_primitives::keccak256(expected_raw).0
        );
    }

    #[test]
    fn inner_action_tree_proofs_share_the_committed_root() {
        let leaves = [[0x51; 32], [0x52; 32], [0x53; 32]];
        let root = inner_action_commitment::root(&leaves);
        for (index, leaf) in leaves.iter().copied().enumerate() {
            let proof = inner_action_commitment::merkle_proof(&leaves, index).unwrap();
            assert!(inner_action_commitment::verify_merkle_proof(
                leaf, index, &proof, root
            ));
        }
        assert!(inner_action_commitment::merkle_proof(&leaves, leaves.len()).is_none());
    }

    #[test]
    fn bridge_public_values_v2_round_trip_and_rejects_tail_drift() {
        let values = BridgeTransitionPublicValuesV2 {
            ethereum_state_before: [0x11; 32],
            ethereum_state_after: [0x22; 32],
            ethereum_nonce_before: 4,
            ethereum_nonce_after: 6,
            zeko_action_state_before: [0x33; 32],
            zeko_action_state_after: [0x66; 32],
            zeko_action_state_length_before: 9,
            zeko_action_state_length_after: 11,
            actions: vec![
                BridgeOuterActionV2 {
                    fields: [[0x44; 32]; BRIDGE_ACTION_FIELDS],
                    state_after: [0x55; 32],
                },
                BridgeOuterActionV2 {
                    fields: [[0x77; 32]; BRIDGE_ACTION_FIELDS],
                    state_after: [0x66; 32],
                },
            ],
        };
        let encoded = values.encode();
        assert_eq!(
            encoded.len(),
            BRIDGE_PUBLIC_VALUES_V2_HEADER_LENGTH + 2 * BRIDGE_ACTION_V2_LENGTH
        );
        assert_eq!(
            BridgeTransitionPublicValuesV2::decode(&encoded).unwrap(),
            values
        );

        let mut truncated = encoded.clone();
        truncated.pop();
        assert!(BridgeTransitionPublicValuesV2::decode(&truncated).is_err());

        let mut bad_count = encoded;
        bad_count[160..164].copy_from_slice(&3u32.to_be_bytes());
        assert!(BridgeTransitionPublicValuesV2::decode(&bad_count).is_err());
    }
}
