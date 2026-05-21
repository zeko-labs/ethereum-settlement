use mina_p2p_messages::v2::{
    MinaBaseUserCommandStableV2, MinaBaseZkappCommandTStableV1WireStableV1,
    PicklesProofProofsVerified2ReprStableV2,
};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// rkyv structs — used only for static SRS file embedding (include_bytes!)
// ---------------------------------------------------------------------------

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Clone, Debug)]
#[rkyv(derive(Debug))]
pub struct RkyvPoint {
    pub x: [u8; 32],
    pub y: [u8; 32],
    pub infinity: bool,
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Clone, Debug)]
#[rkyv(derive(Debug))]
pub struct RkyvPolyComm {
    pub chunks: Vec<RkyvPoint>,
}

#[derive(Archive, RkyvSerialize, RkyvDeserialize, Clone, Debug)]
#[rkyv(derive(Debug))]
pub struct RkyvSRS {
    pub g_flat: Vec<[u8; 65]>, // 32768 × 65 bytes
    pub h_flat: [u8; 65],
    pub domain_size: usize,
    pub lagrange_flat: Vec<[u8; 65]>, // domain_size × 65 bytes
}

// ---------------------------------------------------------------------------
// Host-only — parsing helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ParsedZkappTransaction {
    pub wire_command: MinaBaseUserCommandStableV2,
    pub zkapp_command: MinaBaseZkappCommandTStableV1WireStableV1,
    pub proof: PicklesProofProofsVerified2ReprStableV2,
}

// ---------------------------------------------------------------------------
// serde — used by sp1_zkvm::io::commit and bincode for host/guest I/O
// ---------------------------------------------------------------------------

/// Account precondition coming from the Solidity smart contract.
/// Each field is Option: None means "no constraint" (wildcard).
/// This will be committed as a public value so Solidity can verify
/// that the guest used the expected preconditions.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountPrecondition {
    /// Balance in nanomina
    pub balance: Option<u64>,
    /// Account nonce
    pub nonce: Option<u32>,
    /// Receipt chain hash as raw bytes
    pub receipt_chain_hash: Option<[u8; 32]>,
    /// Delegate public key (compressed)
    pub delegate: Option<[u8; 32]>,
    /// zkApp state array — 8 field elements as bytes, None = wildcard
    pub state: [Option<[u8; 32]>; 8],
    /// Action state as bytes
    pub action_state: Option<[u8; 32]>,
    /// Whether the account has been proved
    pub proved_state: Option<bool>,
    /// Whether the account is new
    pub is_new: Option<bool>,
}

impl Default for AccountPrecondition {
    fn default() -> Self {
        Self {
            balance: None,
            nonce: None,
            receipt_chain_hash: None,
            delegate: None,
            state: [None; 8],
            action_state: None,
            proved_state: None,
            is_new: None,
        }
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

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SerializablePlonk {
    pub alpha: [u64; 2],
    pub beta: [u64; 2],
    pub gamma: [u64; 2],
    pub zeta: [u64; 2],
    pub zeta_to_srs_length: [u8; 32], // Fp serialisé
    pub zeta_to_domain_size: [u8; 32],
    pub perm: [u8; 32],
    pub lookup: Option<[u64; 2]>,
    pub feature_flags_range_check0: bool,
    pub feature_flags_range_check1: bool,
    pub feature_flags_foreign_field_add: bool,
    pub feature_flags_foreign_field_mul: bool,
    pub feature_flags_xor: bool,
    pub feature_flags_rot: bool,
    pub feature_flags_lookup: bool,
    pub feature_flags_runtime_tables: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SerializableDeferredValues {
    pub plonk: SerializablePlonk,
    pub combined_inner_product: [u8; 32], // Fp serialisé
    pub b: [u8; 32],
    pub xi: [u64; 2],
    pub bulletproof_challenges: Vec<[u8; 32]>, // Vec<Fp> serialisé
    pub branch_data_proofs_verified: u8,       // 0, 1, ou 2
    pub branch_data_domain_log2: u8,
}

pub type Bytes32 = [u8; 32];
pub type Address = [u8; 20];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeDeposit {
    pub token: Address,
    pub amount: Bytes32,
    pub zeko_recipient: Vec<u8>,
    pub zeko_recipient_fields: Vec<Bytes32>,
    pub timeout: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EthereumBridgeState {
    pub chain_id: u64,
    pub bridge_address: Address,
    pub deposit_nonce: u64,
    pub deposit_state: Bytes32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ZekoBridgeState {
    pub action_state: Bytes32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeTransitionInput {
    pub ethereum: EthereumBridgeState,
    pub zeko: ZekoBridgeState,
    pub deposits: Vec<BridgeDeposit>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ResolvedBridgeDeposit {
    pub nonce: u64,
    pub token: Address,
    pub amount: Bytes32,
    pub zeko_amount: Bytes32,
    pub zeko_recipient_hash: Bytes32,
    pub timeout: u64,
    pub ethereum_deposit_leaf: Bytes32,
    pub zeko_action_hash: Bytes32,
    pub zeko_action_list_hash: Bytes32,
    pub zeko_action_state_after: Bytes32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeTransitionPublicValues {
    pub ethereum_state_before: Bytes32,
    pub ethereum_state_after: Bytes32,
    pub ethereum_nonce_before: u64,
    pub ethereum_nonce_after: u64,
    pub zeko_action_state_before: Bytes32,
    pub zeko_action_state_after: Bytes32,
    pub deposit_count: u32,
    pub resolved_deposits: Vec<ResolvedBridgeDeposit>,
}
