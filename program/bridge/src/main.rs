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
    Address, BridgeTransitionInput, BridgeTransitionPublicValues, Bytes32, ZekoAddress,
};

fn main() {
    let input: BridgeTransitionInput = sp1_zkvm::io::read();

    let mut ethereum_state = input.ethereum.deposit_state;
    let mut zeko_action_state = fp_from_bytes(input.zeko.action_state);
    let mut next_nonce = input.ethereum.deposit_nonce;

    let empty_action_list_hash = empty_hash_with_prefix("MinaZkappActionsEmpty");

    for deposit in &input.deposits {
        let (zeko_recipient_x, zeko_recipient_is_odd) = unpack_zeko_address(deposit.zeko_recipient);

        let zeko_amount = u256_from_bytes(deposit.zeko_amount);

        next_nonce += 1;

        let ethereum_deposit_leaf = compute_ethereum_deposit_leaf(
            input.ethereum.chain_id,
            input.ethereum.bridge_address,
            deposit.token,
            deposit.zeko_recipient,
            zeko_amount,
            deposit.timeout,
            next_nonce,
        );

        ethereum_state = compute_ethereum_state(ethereum_state, ethereum_deposit_leaf);

        // L1 outer witness action: [discriminant=1, aux, children_digest, slot_lower, slot_upper]
        let action_fields = compute_zeko_outer_witness_fields(
            input.ethereum.bridge_address,
            zeko_amount,
            zeko_recipient_x,
            zeko_recipient_is_odd,
            deposit.timeout,
            fp_from_bytes(deposit.children_digest),
            deposit.slot_range_lower,
            deposit.slot_range_upper,
        );
        let zeko_action_list_hash = action_list_add_fields(empty_action_list_hash, &action_fields);
        zeko_action_state = merkle_actions_add(zeko_action_state, zeko_action_list_hash);
    }

    sp1_zkvm::io::commit(&BridgeTransitionPublicValues {
        ethereum_state_before: input.ethereum.deposit_state,
        ethereum_state_after: ethereum_state,
        ethereum_nonce_before: input.ethereum.deposit_nonce,
        ethereum_nonce_after: next_nonce,
        zeko_action_state_before: fp_to_bytes(fp_from_bytes(input.zeko.action_state)),
        zeko_action_state_after: fp_to_bytes(zeko_action_state),
        deposit_count: input.deposits.len() as u32,
    });
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
        fp_from_u256(zeko_amount),
        fp_from_u256(zeko_recipient_x),
        Fp::from(zeko_recipient_is_odd as u8),
        Fp::from(timeout),
    ];
    hash_with_prefix("Deposit_params - qFB3jXP*)", &fields)
}

// Returns the 5 action fields for an L1 outer witness (deposit) action:
// [discriminant=1, aux, children_digest, slot_range_lower, slot_range_upper]
fn compute_zeko_outer_witness_fields(
    holder_account_l1: Address,
    zeko_amount: U256,
    zeko_recipient_x: U256,
    zeko_recipient_is_odd: bool,
    timeout: u64,
    children_digest: Fp,
    slot_range_lower: u64,
    slot_range_upper: u64,
) -> [Fp; 5] {
    let aux = compute_deposit_aux(
        holder_account_l1,
        zeko_amount,
        zeko_recipient_x,
        zeko_recipient_is_odd,
        timeout,
    );
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
    Fp::from_be_bytes_mod_order(&bytes)
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
                ["0", "13445954892259151401062147356414539397053929755454089729686468374072224770524", "14544341622324407306183827793073118566432371121764582930297443254361206133838"],
                "20564005778679112305921383783621393576220961645269793062533625001478041817089",
            ),
            (
                "20564005778679112305921383783621393576220961645269793062533625001478041817089",
                ["0", "3418969254967426460902743142395488746910205347512382940433097464676038721351", "14544341622324407306183827793073118566432371121764582930297443254361206133838"],
                "14088641427554771616107512497342397932082101784403114407990069911207727165132",
            ),
            (
                "14088641427554771616107512497342397932082101784403114407990069911207727165132",
                ["0", "3418969254967426460902743142395488746910205347512382940433097464676038721351", "14544341622324407306183827793073118566432371121764582930297443254361206133838"],
                "5592644305669396735852728084598993836947101033485055082318992298663200236730",
            ),
            (
                "5592644305669396735852728084598993836947101033485055082318992298663200236730",
                ["0", "7290175672191916634614598157462226143709763480793909565940809202163511105802", "14544341622324407306183827793073118566432371121764582930297443254361206133838"],
                "7230675077846107971049681873539601135350652909070232374148538403307839283596",
            ),
            (
                "7230675077846107971049681873539601135350652909070232374148538403307839283596",
                ["0", "23481682909396816666298220553789953254792289472463233634030406696841084292644", "7293853241236284976483542027714912722616630571844677510574672951635140291085"],
                "23345261943210583986479677938738582339161417082508992471536919886924203109093",
            ),
            (
                "23345261943210583986479677938738582339161417082508992471536919886924203109093",
                ["0", "19783371664972363249023705802644483010603479698004347610850670392839625052708", "14544341622324407306183827793073118566432371121764582930297443254361206133838"],
                "18067506367558727641677130278527360334316654990876259625674197924704612602695",
            ),
            (
                "18067506367558727641677130278527360334316654990876259625674197924704612602695",
                ["0", "19783371664972363249023705802644483010603479698004347610850670392839625052708", "14544341622324407306183827793073118566432371121764582930297443254361206133838"],
                "2746959157610027380951551944033406547038529271116301057152331276522725315733",
            ),
            (
                "2746959157610027380951551944033406547038529271116301057152331276522725315733",
                ["0", "27834258681202107734246517626480949164201501965735911700310484065477580173610", "14544341622324407306183827793073118566432371121764582930297443254361206133838"],
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
        let before = "14869234878481883326787311116385242007710904539061722321273218971438489367544";
        let expected_after = "20470932486817125004352886658008606971240848472715441072030772621176842217909";

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

    /// Legacy fixture test (simplified 1-field model from action-state.ts).
    /// NOTE: this uses a simplified bridge contract that dispatches only `aux`
    /// as a single Field, not the full 5-field outer witness structure.
    /// Kept for reference — the real Zeko bridge uses real_l1_outer_witness_matches_onchain_state.
    #[test]
    fn fixture_deposit_matches_zeko_action_state() {
        let mut bridge_address = [0u8; 20];
        bridge_address[19] = 1;

        let deposits = [
            (
                U256::from(1_000_000_000u64),
                hex32("0000000000000000000000000000000000000000000000000000000001020304"),
                hex32("08c18c1e345342a9376a5196008a3c2a47c9c544215e26594d3a7bf64a7c53b8"),
            ),
            (
                U256::from(2_000_000_000u64),
                hex32("0000000000000000000000000000000000000000000000000000000005060708"),
                hex32("2b27eaae27d23ace717a80ad95f889a5977f5c278f158e6a6adda717e6a870c7"),
            ),
            (
                U256::from(3_000_000_000u64),
                hex32("80000000000000000000000000000000000000000000000000000000090a0b0c"),
                hex32("2b8061d0b565f80c99acf967a3402618deecf886865394b67818fa988428f020"),
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
                3600,
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
            hex32("3d638b908c4241e7b417d1790a79d0fe3277a133a5a87e12a484cd756de795bf")
        );
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
