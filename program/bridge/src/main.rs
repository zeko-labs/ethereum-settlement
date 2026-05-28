#![no_main]
sp1_zkvm::entrypoint!(main);

use alloy_primitives::{keccak256, U256};
use ark_ff::PrimeField;
use ark_serialize::CanonicalSerialize;
use mina_curves::pasta::{Fp, PallasParameters};
use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::pasta::{fp_kimchi, FULL_ROUNDS};
use mina_poseidon::sponge::DefaultFqSponge;
use mina_poseidon::FqSponge;
use zeko_sp1_lib::{
    Address, BridgeTransitionInput, BridgeTransitionPublicValues, Bytes32, ResolvedBridgeDeposit,
    ZekoAddress,
};

type ActionStateSponge = DefaultFqSponge<PallasParameters, PlonkSpongeConstantsKimchi, FULL_ROUNDS>;

fn main() {
    let input: BridgeTransitionInput = sp1_zkvm::io::read();

    let mut ethereum_state = input.ethereum.deposit_state;
    let mut zeko_action_state = fp_from_bytes(input.zeko.action_state);
    let mut next_nonce = input.ethereum.deposit_nonce;
    let mut resolved_deposits = Vec::with_capacity(input.deposits.len());

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

        resolved_deposits.push(ResolvedBridgeDeposit {
            nonce: next_nonce,
            token: deposit.token,
            amount: deposit.amount,
            zeko_amount: u256_to_bytes(zeko_amount),
            zeko_recipient: deposit.zeko_recipient,
            timeout: deposit.timeout,
            ethereum_deposit_leaf,
            zeko_action_hash: fp_to_bytes(zeko_action_hash),
            zeko_action_list_hash: fp_to_bytes(zeko_action_list_hash),
            zeko_action_state_after: fp_to_bytes(zeko_action_state),
        });
    }

    sp1_zkvm::io::commit(&BridgeTransitionPublicValues {
        ethereum_state_before: input.ethereum.deposit_state,
        ethereum_state_after: ethereum_state,
        ethereum_nonce_before: input.ethereum.deposit_nonce,
        ethereum_nonce_after: next_nonce,
        zeko_action_state_before: fp_to_bytes(fp_from_bytes(input.zeko.action_state)),
        zeko_action_state_after: fp_to_bytes(zeko_action_state),
        deposit_count: input.deposits.len() as u32,
        resolved_deposits,
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

fn action_list_add(hash: Fp, action: Fp) -> Fp {
    let event_hash = hash_with_prefix("MinaZkappEvent******", &[action]);
    hash_with_prefix("MinaZkappSeqEvents**", &[hash, event_hash])
}

fn merkle_actions_add(hash: Fp, actions_hash: Fp) -> Fp {
    hash_with_prefix("MinaZkappSeqEvents**", &[hash, actions_hash])
}

fn empty_hash_with_prefix(prefix: &str) -> Fp {
    salt(prefix)
}

fn hash_with_prefix(prefix: &str, input: &[Fp]) -> Fp {
    let mut sponge = new_action_state_sponge();
    let prefixed = prefix_to_field(prefix);
    sponge.absorb_fq(&[prefixed]);
    if !input.is_empty() {
        sponge.absorb_fq(input);
    }
    sponge.digest_fq()
}

fn salt(prefix: &str) -> Fp {
    let mut sponge = new_action_state_sponge();
    sponge.absorb_fq(&[prefix_to_field(prefix)]);
    sponge.digest_fq()
}

fn new_action_state_sponge() -> ActionStateSponge {
    ActionStateSponge::new(fp_kimchi::static_params())
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
