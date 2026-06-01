use mina_p2p_messages::v2::{
    MinaBaseUserCommandStableV2, MinaBaseZkappCommandTStableV1WireStableV1,
    PicklesProofProofsVerified2ReprStableV2,
};
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize};
use std::fmt;

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
pub type ZekoAddress = Bytes32;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeDeposit {
    #[serde(with = "serde_address")]
    pub token: Address,
    #[serde(with = "serde_bytes32")]
    pub amount: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub zeko_amount: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub zeko_recipient: ZekoAddress,
    pub timeout: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeWithdraw {
    #[serde(with = "serde_bytes32")]
    pub token: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub recipient: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub amount: Bytes32,
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
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeTransitionInput {
    pub ethereum: EthereumBridgeState,
    pub zeko: ZekoBridgeState,
    pub deposits: Vec<BridgeDeposit>,
    pub withdraws: Vec<BridgeWithdraw>,
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
    #[serde(with = "serde_bytes32")]
    pub ethereum_withdraw_state_before: Bytes32,
    #[serde(with = "serde_bytes32")]
    pub ethereum_withdraw_state_after: Bytes32,
    pub deposit_count: u32,
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
