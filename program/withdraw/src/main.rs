#![cfg_attr(not(test), no_main)]
#[cfg(not(test))]
sp1_zkvm::entrypoint!(main);

use alloy_primitives::keccak256;
use ark_ff::PrimeField;
use ark_serialize::CanonicalSerialize;
use mina_curves::pasta::Fp;
use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::pasta::{fp_kimchi, FULL_ROUNDS};
use mina_poseidon::permutation::poseidon_block_cipher;
use zeko_sp1_lib::{Address, Bytes32, WithdrawTransitionInput, WithdrawTransitionPublicValues};

const WITHDRAW_TREE_DEPTH: usize = 16;
const MAX_WITHDRAWS: usize = 1 << WITHDRAW_TREE_DEPTH;

fn main() {
    let input: WithdrawTransitionInput = sp1_zkvm::io::read();
    let public_values = process_withdraw_transition(&input);
    sp1_zkvm::io::commit(&public_values);
}

fn process_withdraw_transition(input: &WithdrawTransitionInput) -> WithdrawTransitionPublicValues {
    assert!(
        input.withdraws.len() <= MAX_WITHDRAWS,
        "too many withdrawals"
    );

    let mut ethereum_withdraw_state = input.ethereum.withdraw_state;
    let mut zeko_action_state = fp_from_bytes(input.zeko.action_state);
    let mut ethereum_withdraw_leaves = Vec::with_capacity(input.withdraws.len());

    let empty_action_list_hash = empty_hash_with_prefix("MinaZkappActionsEmpty");

    for withdraw in &input.withdraws {
        // TODO: Remove this assert once Zeko withdrawal actions support token identifiers.
        assert_eq!(withdraw.token, [0u8; 32], "only native token supported");

        let ethereum_withdraw_leaf = compute_ethereum_withdraw_leaf(
            input.ethereum.chain_id,
            input.ethereum.bridge_address,
            withdraw.token,
            withdraw.recipient,
            withdraw.amount,
        );

        ethereum_withdraw_leaves.push(ethereum_withdraw_leaf);
        ethereum_withdraw_state =
            compute_ethereum_withdraw_state(ethereum_withdraw_state, ethereum_withdraw_leaf);

        let zeko_action_hash =
            compute_zeko_withdraw_action_hash(withdraw.recipient, withdraw.amount);
        let zeko_action_list_hash = action_list_add(empty_action_list_hash, zeko_action_hash);
        zeko_action_state = merkle_actions_add(zeko_action_state, zeko_action_list_hash);
    }

    WithdrawTransitionPublicValues {
        zeko_action_state_before: fp_to_bytes(fp_from_bytes(input.zeko.action_state)),
        zeko_action_state_after: fp_to_bytes(zeko_action_state),
        ethereum_withdraw_state_before: input.ethereum.withdraw_state,
        ethereum_withdraw_state_after: ethereum_withdraw_state,
        withdrawal_root: compute_withdrawal_root(&ethereum_withdraw_leaves),
        withdraw_count: input.withdraws.len() as u32,
    }
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

fn compute_withdrawal_root(leaves: &[Bytes32]) -> Bytes32 {
    assert!(leaves.len() <= MAX_WITHDRAWS, "too many withdrawals");

    let zero_hashes = compute_zero_hashes();
    if leaves.is_empty() {
        return zero_hashes[WITHDRAW_TREE_DEPTH];
    }

    let mut nodes = leaves.to_vec();
    for level in 0..WITHDRAW_TREE_DEPTH {
        let mut parents = Vec::with_capacity(nodes.len().div_ceil(2));
        for pair in nodes.chunks(2) {
            let right = if pair.len() == 2 {
                pair[1]
            } else {
                zero_hashes[level]
            };
            parents.push(hash_merkle_node(pair[0], right));
        }
        nodes = parents;
    }

    assert_eq!(nodes.len(), 1, "invalid withdrawal tree");
    nodes[0]
}

fn compute_zero_hashes() -> [Bytes32; WITHDRAW_TREE_DEPTH + 1] {
    let mut zero_hashes = [[0u8; 32]; WITHDRAW_TREE_DEPTH + 1];
    for level in 0..WITHDRAW_TREE_DEPTH {
        zero_hashes[level + 1] = hash_merkle_node(zero_hashes[level], zero_hashes[level]);
    }
    zero_hashes
}

fn hash_merkle_node(left: Bytes32, right: Bytes32) -> Bytes32 {
    let mut encoded = Vec::with_capacity(96);
    encoded.extend_from_slice(&keccak256("ZEKO_BRIDGE_WITHDRAW_MERKLE_NODE_V1".as_bytes()).0);
    encoded.extend_from_slice(&left);
    encoded.extend_from_slice(&right);
    keccak256(encoded).0
}

fn compute_zeko_withdraw_action_hash(recipient: Bytes32, amount: Bytes32) -> Fp {
    let fields = [
        Fp::from(0u8),
        fp_from_bytes(amount),
        fp_from_bytes(recipient),
    ];

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

#[cfg(test)]
mod tests {
    use super::*;
    use zeko_sp1_lib::{
        BridgeWithdraw, EthereumBridgeState, WithdrawTransitionInput, ZekoBridgeState,
    };

    #[test]
    fn withdrawal_hash_matches_zeko_aux_shape() {
        let recipient = hex32("0000000000000000000000000000000000000000000000000000000001020304");
        let amount = hex32("000000000000000000000000000000000000000000000000000000003b9aca00");

        let action_hash = compute_zeko_withdraw_action_hash(recipient, amount);
        let expected = hash_with_prefix(
            "Withdrawal_params - qFB3jXP*)",
            &[
                Fp::from(0u8),
                fp_from_bytes(amount),
                fp_from_bytes(recipient),
            ],
        );

        assert_eq!(action_hash, expected);
    }

    #[test]
    fn empty_withdrawal_root_is_fully_padded_tree() {
        assert_eq!(
            compute_withdrawal_root(&[]),
            compute_zero_hashes()[WITHDRAW_TREE_DEPTH]
        );
    }

    #[test]
    fn one_withdrawal_root_uses_zero_siblings() {
        let leaf = hex32("0000000000000000000000000000000000000000000000000000000000000001");
        let zero_hashes = compute_zero_hashes();
        let mut expected = leaf;
        for sibling in zero_hashes.iter().take(WITHDRAW_TREE_DEPTH) {
            expected = hash_merkle_node(expected, *sibling);
        }

        assert_eq!(compute_withdrawal_root(&[leaf]), expected);
    }

    #[test]
    fn two_withdrawal_root_preserves_leaf_order() {
        let left = hex32("0000000000000000000000000000000000000000000000000000000000000001");
        let right = hex32("0000000000000000000000000000000000000000000000000000000000000002");
        let zero_hashes = compute_zero_hashes();
        let mut expected = hash_merkle_node(left, right);
        for sibling in zero_hashes.iter().take(WITHDRAW_TREE_DEPTH).skip(1) {
            expected = hash_merkle_node(expected, *sibling);
        }

        assert_eq!(compute_withdrawal_root(&[left, right]), expected);
        assert_ne!(
            compute_withdrawal_root(&[left, right]),
            compute_withdrawal_root(&[right, left])
        );
    }

    #[test]
    #[should_panic(expected = "too many withdrawals")]
    fn more_than_max_withdrawals_fails() {
        let input = test_input(vec![test_withdraw(1); MAX_WITHDRAWS + 1]);
        process_withdraw_transition(&input);
    }

    #[test]
    #[should_panic(expected = "only native token supported")]
    fn non_native_token_fails() {
        let mut input = test_input(vec![test_withdraw(1)]);
        input.withdraws[0].token[31] = 1;
        process_withdraw_transition(&input);
    }

    #[test]
    fn public_values_include_withdrawal_root_in_expected_order() {
        let input = test_input(vec![test_withdraw(1)]);
        let public_values = process_withdraw_transition(&input);
        let encoded = bincode::serialize(&public_values).expect("serialize public values");

        assert_eq!(encoded.len(), 164);
        assert_eq!(&encoded[128..160], &public_values.withdrawal_root);
        assert_eq!(&encoded[160..164], &1u32.to_le_bytes());
    }

    fn test_input(withdraws: Vec<BridgeWithdraw>) -> WithdrawTransitionInput {
        WithdrawTransitionInput {
            ethereum: EthereumBridgeState {
                chain_id: 1,
                bridge_address: [0u8; 20],
                deposit_nonce: 0,
                deposit_state: [0u8; 32],
                withdraw_state: [0u8; 32],
            },
            zeko: ZekoBridgeState {
                action_state: hex32(
                    "3772bc5435b957f81f86f752e93f2e29e886ac24580b3d1ec879c1dad26965f9",
                ),
            },
            withdraws,
        }
    }

    fn test_withdraw(value: u8) -> BridgeWithdraw {
        let mut recipient = [0u8; 32];
        recipient[31] = value;
        let mut amount = [0u8; 32];
        amount[31] = value;
        BridgeWithdraw {
            token: [0u8; 32],
            recipient,
            amount,
        }
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
