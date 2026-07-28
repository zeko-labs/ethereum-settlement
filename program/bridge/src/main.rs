#![cfg_attr(not(test), no_main)]
#[cfg(not(test))]
sp1_zkvm::entrypoint!(main);

use alloy_primitives::{keccak256, U256};
use ark_ff::PrimeField;
use ark_serialize::CanonicalSerialize;
use mina_curves::pasta::Fp;
use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::pasta::{fp_kimchi, FULL_ROUNDS};
use mina_poseidon::permutation::poseidon_block_cipher;
use zeko_sp1_lib::{
    Address, BridgeOuterActionV2, BridgeTransitionInput, BridgeTransitionPublicValuesV2, Bytes32,
    ZekoAddress, ERC20_ACTION_ENCODING_V1, ERC20_ACTION_ENCODING_V2,
};

const WEI_PER_ZEKO_UNIT: u64 = 1_000_000_000;
const INFINITE_TIMEOUT: u64 = u32::MAX as u64;

fn main() {
    let input: BridgeTransitionInput = sp1_zkvm::io::read();
    let public_values = derive_bridge_transition(input);
    sp1_zkvm::io::commit_slice(&public_values.encode());
}

fn derive_bridge_transition(input: BridgeTransitionInput) -> BridgeTransitionPublicValuesV2 {
    assert!(
        input.deposits.len() <= u32::MAX as usize,
        "too many deposits"
    );
    assert!(!input.deposits.is_empty(), "empty deposit batch");

    let mut ethereum_state = input.ethereum.deposit_state;
    let mut zeko_action_state = fp_from_bytes(input.zeko.action_state);
    let mut next_nonce = input.ethereum.deposit_nonce;
    let mut emitted_actions = Vec::with_capacity(input.deposits.len());

    let empty_action_list_hash = empty_hash_with_prefix("MinaZkappActionsEmpty");

    for deposit in &input.deposits {
        let (zeko_recipient_x, zeko_recipient_is_odd) = unpack_zeko_address(deposit.zeko_recipient);

        let ethereum_amount = u256_from_bytes(deposit.amount);
        assert!(ethereum_amount > U256::ZERO, "zero deposit");
        let is_native = deposit.token == [0u8; 20];
        let zeko_amount = if is_native {
            assert_eq!(deposit.asset_id, [0u8; 32], "native asset id must be zero");
            assert_eq!(
                deposit.registry_index, 0,
                "native deposit has a registry index"
            );
            assert_eq!(
                deposit.record_commitment, [0u8; 32],
                "native deposit has a record commitment"
            );
            assert_eq!(
                ethereum_amount % U256::from(WEI_PER_ZEKO_UNIT),
                U256::ZERO,
                "native deposit must have 1 gwei granularity"
            );
            let amount = ethereum_amount / U256::from(WEI_PER_ZEKO_UNIT);
            assert!(
                amount <= U256::from(u64::MAX),
                "native deposit exceeds Mina amount"
            );
            if let Some(supplied) = deposit.zeko_amount {
                assert_eq!(amount, U256::from(supplied), "native Zeko amount mismatch");
            }
            amount
        } else {
            assert_ne!(
                deposit.asset_id, [0u8; 32],
                "ERC20 asset id must be non-zero"
            );
            assert_eq!(
                deposit.timeout, INFINITE_TIMEOUT,
                "canonical ERC20 deposit must use infinite timeout"
            );
            match deposit.encoding_version {
                ERC20_ACTION_ENCODING_V1 => {
                    assert_eq!(
                        deposit.registry_index, 0,
                        "legacy ERC20 deposit has a registry index"
                    );
                    assert_eq!(
                        deposit.record_commitment, [0u8; 32],
                        "legacy ERC20 deposit has a record commitment"
                    );
                }
                ERC20_ACTION_ENCODING_V2 => {
                    assert_ne!(
                        deposit.record_commitment, [0u8; 32],
                        "registry ERC20 deposit record commitment is zero"
                    );
                    fp_from_bytes(deposit.record_commitment);
                }
                version => panic!("unsupported ERC20 deposit encoding version {version}"),
            }
            assert!(
                ethereum_amount <= U256::from(u64::MAX),
                "ERC20 deposit exceeds Mina amount"
            );
            let supplied = deposit
                .zeko_amount
                .expect("canonical ERC20 deposit is missing Zeko amount");
            assert_eq!(
                ethereum_amount,
                U256::from(supplied),
                "canonical ERC20 deposit must preserve base units"
            );
            U256::from(supplied)
        };

        next_nonce += 1;

        let ethereum_deposit_leaf = if is_native {
            compute_ethereum_deposit_leaf(
                input.ethereum.chain_id,
                input.ethereum.bridge_address,
                deposit.token,
                deposit.zeko_recipient,
                zeko_amount,
                deposit.timeout,
                next_nonce,
            )
        } else {
            match deposit.encoding_version {
                ERC20_ACTION_ENCODING_V1 => compute_ethereum_erc20_deposit_leaf_v1(
                    input.ethereum.chain_id,
                    input.ethereum.bridge_address,
                    deposit.token,
                    deposit.asset_id,
                    deposit.zeko_recipient,
                    zeko_amount,
                    deposit.timeout,
                    next_nonce,
                ),
                ERC20_ACTION_ENCODING_V2 => compute_ethereum_erc20_deposit_leaf_v2(
                    input.ethereum.chain_id,
                    input.ethereum.bridge_address,
                    deposit.token,
                    deposit.registry_index,
                    deposit.record_commitment,
                    deposit.asset_id,
                    deposit.zeko_recipient,
                    zeko_amount,
                    deposit.timeout,
                    next_nonce,
                ),
                _ => unreachable!("ERC20 action version checked above"),
            }
        };

        ethereum_state = compute_ethereum_state(ethereum_state, ethereum_deposit_leaf);

        // L1 outer witness action: [discriminant=1, aux, children_digest, slot_lower, slot_upper]
        let aux = if is_native {
            compute_deposit_aux(
                input.ethereum.bridge_address,
                zeko_amount,
                zeko_recipient_x,
                zeko_recipient_is_odd,
                deposit.timeout,
            )
        } else {
            match deposit.encoding_version {
                ERC20_ACTION_ENCODING_V1 => compute_erc20_deposit_aux_v1(
                    deposit.asset_id,
                    input.ethereum.bridge_address,
                    zeko_amount,
                    zeko_recipient_x,
                    zeko_recipient_is_odd,
                    deposit.timeout,
                ),
                ERC20_ACTION_ENCODING_V2 => compute_erc20_deposit_aux_v2(
                    deposit.registry_index,
                    deposit.record_commitment,
                    deposit.asset_id,
                    input.ethereum.bridge_address,
                    zeko_amount,
                    zeko_recipient_x,
                    zeko_recipient_is_odd,
                    deposit.timeout,
                ),
                _ => unreachable!("ERC20 action version checked above"),
            }
        };
        let action_fields =
            compute_zeko_outer_witness_fields(aux, Fp::from(0u8), 0, INFINITE_TIMEOUT);
        let zeko_action_list_hash = action_list_add_fields(empty_action_list_hash, &action_fields);
        zeko_action_state = merkle_actions_add(zeko_action_state, zeko_action_list_hash);
        emitted_actions.push(BridgeOuterActionV2 {
            fields: action_fields.map(fp_to_bytes),
            state_after: fp_to_bytes(zeko_action_state),
        });
    }

    BridgeTransitionPublicValuesV2 {
        ethereum_state_before: input.ethereum.deposit_state,
        ethereum_state_after: ethereum_state,
        ethereum_nonce_before: input.ethereum.deposit_nonce,
        ethereum_nonce_after: next_nonce,
        zeko_action_state_before: fp_to_bytes(fp_from_bytes(input.zeko.action_state)),
        zeko_action_state_after: fp_to_bytes(zeko_action_state),
        zeko_action_state_length_before: input.zeko.action_state_length,
        zeko_action_state_length_after: input
            .zeko
            .action_state_length
            .checked_add(input.deposits.len() as u32)
            .expect("outer action-state length overflow"),
        actions: emitted_actions,
    }
}

fn compute_ethereum_deposit_leaf(
    chain_id: u64,
    bridge_address: Address,
    token: Address,
    zeko_recipient: ZekoAddress,
    zeko_amount: U256,
    timeout: u64,
    nonce: u64,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(32 * 8);
    encoded.extend_from_slice(&keccak256("ZEKO_BRIDGE_DEPOSIT_LEAF_V1".as_bytes()).0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge_address));
    encoded.extend_from_slice(&address_word(token));
    encoded.extend_from_slice(&zeko_recipient);
    encoded.extend_from_slice(&u256_to_bytes(zeko_amount));
    encoded.extend_from_slice(&u64_word(timeout));
    encoded.extend_from_slice(&u64_word(nonce));
    keccak256(encoded).0
}

fn compute_ethereum_state(previous_state: Bytes32, deposit_leaf: Bytes32) -> Bytes32 {
    let mut encoded = Vec::with_capacity(96);
    encoded.extend_from_slice(&keccak256("ZEKO_BRIDGE_DEPOSIT_STATE_V1".as_bytes()).0);
    encoded.extend_from_slice(&previous_state);
    encoded.extend_from_slice(&deposit_leaf);
    keccak256(encoded).0
}

fn compute_ethereum_erc20_deposit_leaf_v1(
    chain_id: u64,
    bridge_address: Address,
    token: Address,
    asset_id: Bytes32,
    zeko_recipient: ZekoAddress,
    zeko_amount: U256,
    timeout: u64,
    nonce: u64,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(32 * 9);
    encoded.extend_from_slice(&keccak256("ZEKO_ERC20_DEPOSIT_LEAF_V2".as_bytes()).0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge_address));
    encoded.extend_from_slice(&address_word(token));
    encoded.extend_from_slice(&asset_id);
    encoded.extend_from_slice(&zeko_recipient);
    encoded.extend_from_slice(&u256_to_bytes(zeko_amount));
    encoded.extend_from_slice(&u64_word(timeout));
    encoded.extend_from_slice(&u64_word(nonce));
    keccak256(encoded).0
}

#[allow(clippy::too_many_arguments)]
fn compute_ethereum_erc20_deposit_leaf_v2(
    chain_id: u64,
    bridge_address: Address,
    token: Address,
    registry_index: u32,
    record_commitment: Bytes32,
    asset_id: Bytes32,
    zeko_recipient: ZekoAddress,
    zeko_amount: U256,
    timeout: u64,
    nonce: u64,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(32 * 12);
    encoded.extend_from_slice(&keccak256("ZEKO_ERC20_DEPOSIT_LEAF_V3".as_bytes()).0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge_address));
    encoded.extend_from_slice(&address_word(token));
    encoded.extend_from_slice(&u32_word(ERC20_ACTION_ENCODING_V2));
    encoded.extend_from_slice(&u32_word(registry_index));
    encoded.extend_from_slice(&record_commitment);
    encoded.extend_from_slice(&asset_id);
    encoded.extend_from_slice(&zeko_recipient);
    encoded.extend_from_slice(&u256_to_bytes(zeko_amount));
    encoded.extend_from_slice(&u64_word(timeout));
    encoded.extend_from_slice(&u64_word(nonce));
    keccak256(encoded).0
}

fn compute_deposit_aux(
    holder_account_l1: Address,
    zeko_amount: U256,
    zeko_recipient_x: U256,
    zeko_recipient_is_odd: bool,
    timeout: u64,
) -> Fp {
    let fields = [
        Fp::from(0u8), // children = Field(0) for empty call forest
        fp_from_address(holder_account_l1),
        Fp::from(0u8), // synthetic holder compressed-key parity
        fp_from_u256(zeko_amount),
        fp_from_u256(zeko_recipient_x),
        Fp::from(zeko_recipient_is_odd as u8),
        Fp::from(timeout),
    ];
    hash_with_prefix("Ethereum deposit V1", &fields)
}

fn compute_erc20_deposit_aux_v1(
    asset_id: Bytes32,
    holder_account_l1: Address,
    zeko_amount: U256,
    zeko_recipient_x: U256,
    zeko_recipient_is_odd: bool,
    timeout: u64,
) -> Fp {
    let asset_high = U256::from_be_slice(&asset_id[..16]);
    let asset_low = U256::from_be_slice(&asset_id[16..]);
    let fields = [
        fp_from_u256(asset_high),
        fp_from_u256(asset_low),
        Fp::from(0u8), // children = Field(0) for empty call forest
        fp_from_address(holder_account_l1),
        Fp::from(0u8), // synthetic holder compressed-key parity
        fp_from_u256(zeko_amount),
        fp_from_u256(zeko_recipient_x),
        Fp::from(zeko_recipient_is_odd as u8),
        Fp::from(timeout),
    ];
    hash_with_prefix("Ethereum ERC20 deposit V1", &fields)
}

#[allow(clippy::too_many_arguments)]
fn compute_erc20_deposit_aux_v2(
    registry_index: u32,
    record_commitment: Bytes32,
    asset_id: Bytes32,
    holder_account_l1: Address,
    zeko_amount: U256,
    zeko_recipient_x: U256,
    zeko_recipient_is_odd: bool,
    timeout: u64,
) -> Fp {
    let asset_high = U256::from_be_slice(&asset_id[..16]);
    let asset_low = U256::from_be_slice(&asset_id[16..]);
    let fields = [
        Fp::from(ERC20_ACTION_ENCODING_V2),
        Fp::from(registry_index),
        fp_from_bytes(record_commitment),
        fp_from_u256(asset_high),
        fp_from_u256(asset_low),
        Fp::from(0u8), // children = Field(0) for empty call forest
        fp_from_address(holder_account_l1),
        Fp::from(0u8), // synthetic holder compressed-key parity
        fp_from_u256(zeko_amount),
        fp_from_u256(zeko_recipient_x),
        Fp::from(zeko_recipient_is_odd as u8),
        Fp::from(timeout),
    ];
    hash_with_prefix("Ethereum ERC20 deposit V2", &fields)
}

// Returns the 5 action fields for an L1 outer witness (deposit) action:
// [discriminant=1, aux, children_digest, slot_range_lower, slot_range_upper]
fn compute_zeko_outer_witness_fields(
    aux: Fp,
    children_digest: Fp,
    slot_range_lower: u64,
    slot_range_upper: u64,
) -> [Fp; 5] {
    [
        Fp::from(1u8), // discriminant: witness (vs 0 for commit)
        aux,
        children_digest,
        Fp::from(slot_range_lower),
        Fp::from(slot_range_upper),
    ]
}

fn action_list_add_fields(list_hash: Fp, action_fields: &[Fp]) -> Fp {
    let event_hash = hash_with_prefix("MinaZkappEvent******", action_fields);
    hash_with_prefix("MinaZkappSeqEvents**", &[list_hash, event_hash])
}

fn merkle_actions_add(hash: Fp, actions_hash: Fp) -> Fp {
    hash_with_prefix("MinaZkappSeqEvents**", &[hash, actions_hash])
}

fn empty_hash_with_prefix(prefix: &str) -> Fp {
    poseidon_update(
        [Fp::from(0u8), Fp::from(0u8), Fp::from(0u8)],
        &[prefix_to_field(prefix)],
    )[0]
}

fn hash_with_prefix(prefix: &str, input: &[Fp]) -> Fp {
    let init = poseidon_update(
        [Fp::from(0u8), Fp::from(0u8), Fp::from(0u8)],
        &[prefix_to_field(prefix)],
    );
    poseidon_update(init, input)[0]
}

fn poseidon_update(mut state: [Fp; 3], input: &[Fp]) -> [Fp; 3] {
    if input.is_empty() {
        poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi, FULL_ROUNDS>(
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
        poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi, FULL_ROUNDS>(
            fp_kimchi::static_params(),
            &mut state,
        );
    }

    state
}

fn prefix_to_field(prefix: &str) -> Fp {
    assert!(prefix.len() < 32, "prefix too long");
    let mut bytes = [0u8; 32];
    bytes[..prefix.len()].copy_from_slice(prefix.as_bytes());
    Fp::from_le_bytes_mod_order(&bytes)
}

fn fp_from_address(address: Address) -> Fp {
    let mut bytes = [0u8; 32];
    bytes[12..32].copy_from_slice(&address);
    Fp::from_be_bytes_mod_order(&bytes)
}

fn fp_from_u256(value: U256) -> Fp {
    Fp::from_be_bytes_mod_order(&value.to_be_bytes::<32>())
}

fn fp_from_bytes(bytes: Bytes32) -> Fp {
    let value = Fp::from_be_bytes_mod_order(&bytes);
    assert_eq!(fp_to_bytes(value), bytes, "non-canonical Mina field");
    value
}

fn fp_to_bytes(x: Fp) -> Bytes32 {
    let mut buf = [0u8; 32];
    x.serialize_uncompressed(&mut buf[..])
        .expect("serialize field");
    buf.reverse();
    buf
}

fn u64_word(value: u64) -> Bytes32 {
    let mut word = [0u8; 32];
    word[24..32].copy_from_slice(&value.to_be_bytes());
    word
}

fn u32_word(value: u32) -> Bytes32 {
    let mut word = [0u8; 32];
    word[28..32].copy_from_slice(&value.to_be_bytes());
    word
}

fn address_word(address: Address) -> Bytes32 {
    let mut word = [0u8; 32];
    word[12..32].copy_from_slice(&address);
    word
}

fn u256_from_bytes(bytes: Bytes32) -> U256 {
    U256::from_be_slice(&bytes)
}

fn u256_to_bytes(value: U256) -> Bytes32 {
    value.to_be_bytes::<32>()
}

fn unpack_zeko_address(address: ZekoAddress) -> (U256, bool) {
    let x = U256::from_be_slice(&address) & ((U256::from(1u8) << 255) - U256::from(1u8));
    let is_odd = (address[0] & 0x80) != 0;
    let field_order = U256::from_be_slice(&[
        0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x22, 0x46, 0x98, 0xfc, 0x09, 0x4c, 0xf9, 0x1b, 0x99, 0x2d, 0x30, 0xed, 0x00, 0x00,
        0x00, 0x01,
    ]);

    assert!(x < field_order, "invalid zeko address field");

    (x, is_odd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp_from_decimal(s: &str) -> Fp {
        // Parse decimal string into big-endian bytes manually, then into Fp
        let mut out = [0u8; 32];
        for digit in s.bytes() {
            let d = digit - b'0';
            let mut carry = d as u16;
            for byte in out.iter_mut().rev() {
                let next = (*byte as u16) * 10 + carry;
                *byte = next as u8;
                carry = next >> 8;
            }
        }
        Fp::from_be_bytes_mod_order(&out)
    }

    fn fp_to_decimal(x: Fp) -> String {
        let mut buf = [0u8; 32];
        x.serialize_uncompressed(&mut buf[..]).unwrap();
        buf.reverse();
        // big-endian bytes to decimal string
        let mut digits = vec![0u8];
        for byte in &buf {
            let mut carry = *byte as u16;
            for d in digits.iter_mut().rev() {
                let cur = (*d as u16) * 256 + carry;
                *d = (cur % 10) as u8;
                carry = cur / 10;
            }
            while carry > 0 {
                digits.insert(0, (carry % 10) as u8);
                carry /= 10;
            }
        }
        digits.iter().map(|d| (b'0' + d) as char).collect()
    }

    // Compute the action list hash for an action with N fields
    fn action_list_add_fields(list_hash: Fp, fields: &[Fp]) -> Fp {
        let event_hash = hash_with_prefix("MinaZkappEvent******", fields);
        hash_with_prefix("MinaZkappSeqEvents**", &[list_hash, event_hash])
    }

    /// Replays 8 real L2 inner actions fetched from testnet.zeko.io
    /// and verifies that the 3-field action hash formula reproduces
    /// the on-chain action state transitions.
    #[test]
    fn real_l2_inner_actions_match_onchain_state() {
        // Data from: POST https://testnet.zeko.io/graphql
        // Contract: B62qjDedeP9617oTUeN8JGhdiqWg4t64NtQkHaoZB9wyvgSjAyupPU1
        // Each entry: (before_state, [f0, f1, f2], after_state)
        let actions: &[(&str, [&str; 3], &str)] = &[
            (
                "5338488511538591704321908497453393465896611676572626889890352515639793324972",
                [
                    "0",
                    "13445954892259151401062147356414539397053929755454089729686468374072224770524",
                    "14544341622324407306183827793073118566432371121764582930297443254361206133838",
                ],
                "20564005778679112305921383783621393576220961645269793062533625001478041817089",
            ),
            (
                "20564005778679112305921383783621393576220961645269793062533625001478041817089",
                [
                    "0",
                    "3418969254967426460902743142395488746910205347512382940433097464676038721351",
                    "14544341622324407306183827793073118566432371121764582930297443254361206133838",
                ],
                "14088641427554771616107512497342397932082101784403114407990069911207727165132",
            ),
            (
                "14088641427554771616107512497342397932082101784403114407990069911207727165132",
                [
                    "0",
                    "3418969254967426460902743142395488746910205347512382940433097464676038721351",
                    "14544341622324407306183827793073118566432371121764582930297443254361206133838",
                ],
                "5592644305669396735852728084598993836947101033485055082318992298663200236730",
            ),
            (
                "5592644305669396735852728084598993836947101033485055082318992298663200236730",
                [
                    "0",
                    "7290175672191916634614598157462226143709763480793909565940809202163511105802",
                    "14544341622324407306183827793073118566432371121764582930297443254361206133838",
                ],
                "7230675077846107971049681873539601135350652909070232374148538403307839283596",
            ),
            (
                "7230675077846107971049681873539601135350652909070232374148538403307839283596",
                [
                    "0",
                    "23481682909396816666298220553789953254792289472463233634030406696841084292644",
                    "7293853241236284976483542027714912722616630571844677510574672951635140291085",
                ],
                "23345261943210583986479677938738582339161417082508992471536919886924203109093",
            ),
            (
                "23345261943210583986479677938738582339161417082508992471536919886924203109093",
                [
                    "0",
                    "19783371664972363249023705802644483010603479698004347610850670392839625052708",
                    "14544341622324407306183827793073118566432371121764582930297443254361206133838",
                ],
                "18067506367558727641677130278527360334316654990876259625674197924704612602695",
            ),
            (
                "18067506367558727641677130278527360334316654990876259625674197924704612602695",
                [
                    "0",
                    "19783371664972363249023705802644483010603479698004347610850670392839625052708",
                    "14544341622324407306183827793073118566432371121764582930297443254361206133838",
                ],
                "2746959157610027380951551944033406547038529271116301057152331276522725315733",
            ),
            (
                "2746959157610027380951551944033406547038529271116301057152331276522725315733",
                [
                    "0",
                    "27834258681202107734246517626480949164201501965735911700310484065477580173610",
                    "14544341622324407306183827793073118566432371121764582930297443254361206133838",
                ],
                "11066481997049907237147074214507440714257448164444404179272910777489391657254",
            ),
        ];

        let empty = empty_hash_with_prefix("MinaZkappActionsEmpty");

        for (i, (before, fields, expected_after)) in actions.iter().enumerate() {
            let state = fp_from_decimal(before);
            let fps: Vec<Fp> = fields.iter().map(|s| fp_from_decimal(s)).collect();

            // 3-field formula: hash all 3 fields as a single event
            let action_list = action_list_add_fields(empty, &fps);
            let after = merkle_actions_add(state, action_list);

            assert_eq!(
                fp_to_decimal(after),
                *expected_after,
                "action {i}: 3-field formula mismatch\n  got:      {}\n  expected: {}",
                fp_to_decimal(after),
                expected_after,
            );
        }
    }

    /// Verifies a real L1 outer witness action fetched from testnet.api.actions.zeko.io.
    /// Contract: B62qkekmS9273D1EsFfMSJMMDAmgvh1WyoYE2vs1r7k4GtGBqVYABn2
    /// Txn: 5JuHqXG3FuF9EDwQ9BwAYXaAJVDexLsbnuBX6UGVfpsFq24dkkrC
    ///
    /// On-chain fields: ["1", "28349612...", "13465454...", "0", "4294967295"]
    /// The 5-field outer witness formula must reproduce the before→after transition.
    #[test]
    fn real_l1_outer_witness_matches_onchain_state() {
        let before =
            "14869234878481883326787311116385242007710904539061722321273218971438489367544";
        let expected_after =
            "20470932486817125004352886658008606971240848472715441072030772621176842217909";

        // Raw fields from the indexer
        let fields: [&str; 5] = [
            "1",
            "28349612946901459216611267454622531123455255424206629024049044337709921708126",
            "13465454915859917615397187569973631104407941120704862333700387846543210055665",
            "0",
            "4294967295",
        ];

        let state = fp_from_decimal(before);
        let fps: Vec<Fp> = fields.iter().map(|s| fp_from_decimal(s)).collect();
        let empty = empty_hash_with_prefix("MinaZkappActionsEmpty");

        let action_list = action_list_add_fields(empty, &fps);
        let after = merkle_actions_add(state, action_list);

        assert_eq!(
            fp_to_decimal(after),
            expected_after,
            "L1 outer witness 5-field formula mismatch"
        );
    }

    /// Cross-language Ethereum-native deposit vectors. The three aux values
    /// are also asserted by Zeko's `ethereum_bridge_vectors` executable.
    #[test]
    fn fixture_deposit_matches_zeko_action_state() {
        let mut bridge_address = [0u8; 20];
        bridge_address[19] = 1;

        let deposits = [
            (
                U256::from(1_000_000_000u64),
                hex32("0000000000000000000000000000000000000000000000000000000001020304"),
                hex32("2e9d1b29cea8eaba8c1dfe6d8c78b21127ce44a8378b3c9d2ee9ba0ddbd7c849"),
            ),
            (
                U256::from(2_000_000_000u64),
                hex32("0000000000000000000000000000000000000000000000000000000005060708"),
                hex32("1a03b5b4a38e241ee071764a843e5b7bf29aa0e455d7ccd53a83f729885bfb18"),
            ),
            (
                U256::from(3_000_000_000u64),
                hex32("80000000000000000000000000000000000000000000000000000000090a0b0c"),
                hex32("1adc48d4e3b4478369ec2d8ce4ca72c397c9e75f019b24c9d65c262ae9757fa9"),
            ),
        ];

        let mut action_state =
            hex32("3772bc5435b957f81f86f752e93f2e29e886ac24580b3d1ec879c1dad26965f9");

        for (zeko_amount, zeko_recipient, expected_aux) in deposits {
            let (zeko_recipient_x, zeko_recipient_is_odd) = unpack_zeko_address(zeko_recipient);
            let aux = compute_deposit_aux(
                bridge_address,
                zeko_amount,
                zeko_recipient_x,
                zeko_recipient_is_odd,
                INFINITE_TIMEOUT,
            );
            assert_eq!(fp_to_bytes(aux), expected_aux);

            // simplified: dispatch aux as a 1-field action (not the real 5-field structure)
            let action_list_hash =
                action_list_add_fields(empty_hash_with_prefix("MinaZkappActionsEmpty"), &[aux]);
            action_state = fp_to_bytes(merkle_actions_add(
                fp_from_bytes(action_state),
                action_list_hash,
            ));
        }

        assert_eq!(
            action_state,
            hex32("2503022f5ba200b5b44d13741ad0d6e01b8cbdab340d25e637c22f3980be1abf")
        );
    }

    #[test]
    fn erc20_deposit_witness_binds_the_registered_asset() {
        let bridge_address = address(0xb1);
        let token_a = address(0xa1);
        let token_b = address(0xa2);
        let recipient = hex32("0000000000000000000000000000000000000000000000000000000001020304");
        let amount = 2_000_000u64;

        let record_commitment = fp_to_bytes(Fp::from(991u64));
        let input = |token, asset_id, registry_index, record_commitment| BridgeTransitionInput {
            ethereum: zeko_sp1_lib::EthereumBridgeState {
                chain_id: 31337,
                bridge_address,
                deposit_nonce: 0,
                deposit_state: [7u8; 32],
                withdraw_state: [0u8; 32],
            },
            zeko: zeko_sp1_lib::ZekoBridgeState {
                action_state: [9u8; 32],
                action_state_length: 0,
            },
            deposits: vec![zeko_sp1_lib::BridgeDeposit {
                token,
                asset_id,
                encoding_version: ERC20_ACTION_ENCODING_V2,
                registry_index,
                record_commitment,
                amount: u256_to_bytes(U256::from(amount)),
                zeko_amount: Some(amount),
                zeko_recipient: recipient,
                timeout: INFINITE_TIMEOUT,
            }],
        };

        let a = derive_bridge_transition(input(token_a, [0x11; 32], 3, record_commitment));
        let b = derive_bridge_transition(input(token_b, [0x22; 32], 4, record_commitment));

        assert_ne!(a.ethereum_state_after, b.ethereum_state_after);
        assert_ne!(a.actions[0].fields[1], b.actions[0].fields[1]);
        assert_ne!(a.zeko_action_state_after, b.zeko_action_state_after);

        let mut vector_asset = [0u8; 32];
        vector_asset[15] = 1;
        vector_asset[31] = 2;
        let aux = compute_erc20_deposit_aux_v2(
            3,
            record_commitment,
            vector_asset,
            address(1),
            U256::from(amount),
            U256::from(16_909_060u64),
            false,
            INFINITE_TIMEOUT,
        );
        assert_eq!(
            fp_to_bytes(aux),
            hex32("2d60dc7f6f355ec2a3a25a1ecd3da47d4fd77d12038f8a78747fab4e549af1a2")
        );

        let wrong_index =
            derive_bridge_transition(input(token_a, [0x11; 32], 4, record_commitment));
        assert_ne!(a.ethereum_state_after, wrong_index.ethereum_state_after);
        assert_ne!(a.actions[0].fields[1], wrong_index.actions[0].fields[1]);

        let other_commitment = fp_to_bytes(Fp::from(992u64));
        let wrong_commitment =
            derive_bridge_transition(input(token_a, [0x11; 32], 3, other_commitment));
        assert_ne!(
            a.ethereum_state_after,
            wrong_commitment.ethereum_state_after
        );
        assert_ne!(
            a.actions[0].fields[1],
            wrong_commitment.actions[0].fields[1]
        );
    }

    fn address(last: u8) -> Address {
        let mut value = [0u8; 20];
        value[19] = last;
        value
    }

    fn hex32(value: &str) -> [u8; 32] {
        let value = value.strip_prefix("0x").unwrap_or(value);
        assert_eq!(value.len(), 64);

        let bytes = value.as_bytes();
        let mut output = [0u8; 32];
        for i in 0..32 {
            output[i] = (hex_nibble(bytes[i * 2]) << 4) | hex_nibble(bytes[i * 2 + 1]);
        }
        output
    }

    fn hex_nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => panic!("invalid hex byte"),
        }
    }
}
