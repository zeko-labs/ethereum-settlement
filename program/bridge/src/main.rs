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
    let mut ethereum_withdraw_state = input.ethereum.withdraw_state;
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

        let zeko_action_hash = compute_zeko_action_hash(
            input.ethereum.bridge_address,
            zeko_amount,
            zeko_recipient_x,
            zeko_recipient_is_odd,
            deposit.timeout,
        );
        let zeko_action_list_hash = action_list_add(empty_action_list_hash, zeko_action_hash);
        zeko_action_state = merkle_actions_add(zeko_action_state, zeko_action_list_hash);
    }

    for withdraw in &input.withdraws {
        let ethereum_withdraw_leaf = compute_ethereum_withdraw_leaf(
            input.ethereum.chain_id,
            input.ethereum.bridge_address,
            withdraw.token,
            withdraw.recipient,
            withdraw.amount,
        );

        ethereum_withdraw_state =
            compute_ethereum_withdraw_state(ethereum_withdraw_state, ethereum_withdraw_leaf);

        let zeko_action_hash =
            compute_zeko_withdraw_action_hash(withdraw.recipient, withdraw.amount);
        let zeko_action_list_hash = action_list_add(empty_action_list_hash, zeko_action_hash);
        zeko_action_state = merkle_actions_add(zeko_action_state, zeko_action_list_hash);
    }

    sp1_zkvm::io::commit(&BridgeTransitionPublicValues {
        ethereum_state_before: input.ethereum.deposit_state,
        ethereum_state_after: ethereum_state,
        ethereum_nonce_before: input.ethereum.deposit_nonce,
        ethereum_nonce_after: next_nonce,
        zeko_action_state_before: fp_to_bytes(fp_from_bytes(input.zeko.action_state)),
        zeko_action_state_after: fp_to_bytes(zeko_action_state),
        ethereum_withdraw_state_before: input.ethereum.withdraw_state,
        ethereum_withdraw_state_after: ethereum_withdraw_state,
        deposit_count: input.deposits.len() as u32,
        withdraw_count: input.withdraws.len() as u32,
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

fn compute_ethereum_withdraw_leaf(
    chain_id: u64,
    bridge_address: Address,
    token: Bytes32,
    recipient: Bytes32,
    amount: Bytes32,
) -> Bytes32 {
    let mut encoded = Vec::with_capacity(32 * 6);
    encoded.extend_from_slice(&keccak256("ZEKO_BRIDGE_WITHDRAW_LEAF_V1".as_bytes()).0);
    encoded.extend_from_slice(&u64_word(chain_id));
    encoded.extend_from_slice(&address_word(bridge_address));
    encoded.extend_from_slice(&token);
    encoded.extend_from_slice(&recipient);
    encoded.extend_from_slice(&amount);
    keccak256(encoded).0
}

fn compute_ethereum_withdraw_state(previous_state: Bytes32, withdraw_leaf: Bytes32) -> Bytes32 {
    let mut encoded = Vec::with_capacity(96);
    encoded.extend_from_slice(&keccak256("ZEKO_BRIDGE_WITHDRAW_STATE_V1".as_bytes()).0);
    encoded.extend_from_slice(&previous_state);
    encoded.extend_from_slice(&withdraw_leaf);
    keccak256(encoded).0
}

fn compute_zeko_action_hash(
    holder_account_l1: Address,
    zeko_amount: U256,
    zeko_recipient_x: U256,
    zeko_recipient_is_odd: bool,
    timeout: u64,
) -> Fp {
    let mut fields = Vec::with_capacity(6);
    fields.push(Fp::from(0u8));
    fields.push(fp_from_address(holder_account_l1));
    fields.push(fp_from_u256(zeko_amount));
    fields.push(fp_from_u256(zeko_recipient_x));
    fields.push(Fp::from(zeko_recipient_is_odd as u8));
    fields.push(Fp::from(timeout));

    hash_with_prefix("Deposit_params - qFB3jXP*)", &fields)
}

fn compute_zeko_withdraw_action_hash(recipient: Bytes32, amount: Bytes32) -> Fp {
    let mut fields = Vec::with_capacity(3);
    fields.push(Fp::from(1u8));
    fields.push(fp_from_bytes(amount));
    fields.push(fp_from_bytes(recipient));

    hash_with_prefix("Withdrawal_params - qFB3jXP*)", &fields)
}

fn action_list_add(hash: Fp, action: Fp) -> Fp {
    let event_hash = hash_with_prefix("MinaZkappEvent******", &[action]);
    hash_with_prefix("MinaZkappSeqEvents**", &[hash, event_hash])
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

        for (zeko_amount, zeko_recipient, expected_action_hash) in deposits {
            let (zeko_recipient_x, zeko_recipient_is_odd) = unpack_zeko_address(zeko_recipient);
            let action_hash = compute_zeko_action_hash(
                bridge_address,
                zeko_amount,
                zeko_recipient_x,
                zeko_recipient_is_odd,
                3600,
            );
            assert_eq!(fp_to_bytes(action_hash), expected_action_hash);

            let action_list_hash =
                action_list_add(empty_hash_with_prefix("MinaZkappActionsEmpty"), action_hash);
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
