#![no_main]
sp1_zkvm::entrypoint!(main);

use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
use ark_serialize::CanonicalSerialize;
use kimchi::{groupmap::GroupMap, mina_curves::pasta::PallasParameters};
use ledger::proofs::{
    accumulator_check::accumulator_check,
    prover::make_padded_proof_from_p2p,
    transaction::InnerCurve,
    verification::{
        compute_deferred_values, get_message_for_next_step_proof, get_message_for_next_wrap_proof,
        get_prepared_statement, run_checks, VK,
    },
    verifiers::{make_zkapp_verifier_index_with_srs, wrap_domains},
};
use ledger::scan_state::transaction_logic::zkapp_command::{OrIgnore, SetOrKeep, ZkAppCommand};
use ledger::scan_state::transaction_logic::zkapp_statement::ZkappStatement;
use ledger::VerificationKey;
use mina_curves::pasta::{Fp, Fq, Pallas, Vesta};
use mina_p2p_messages::v2::{
    MinaBaseVerificationKeyWireStableV1, MinaBaseZkappCommandTStableV1WireStableV1,
    PicklesProofProofsVerified2ReprStableV2,
};
use mina_poseidon::sponge::{DefaultFqSponge, DefaultFrSponge};
use poly_commitment::{
    hash_map_cache::HashMapCache,
    ipa::{OpeningProof, SRS},
};
use std::{collections::HashMap, sync::Arc};
use zeko_sp1_lib::ArchivedRkyvSRS;

const FULL_ROUNDS: usize = 55;
type SpongeParams = mina_poseidon::constants::PlonkSpongeConstantsKimchi;
type EFqSponge = DefaultFqSponge<PallasParameters, SpongeParams, FULL_ROUNDS>;
type EFrSponge = DefaultFrSponge<Fq, SpongeParams, FULL_ROUNDS>;

#[repr(align(16))]
struct AlignedBytes<const N: usize>([u8; N]);

static PALLAS_SRS_RKYV: AlignedBytes<{ include_bytes!("srs_pallas_kimchi_rkyv.bin").len() }> =
    AlignedBytes(*include_bytes!("srs_pallas_kimchi_rkyv.bin"));

static VESTA_SRS_RKYV: AlignedBytes<{ include_bytes!("srs_vesta_accumulator_rkyv.bin").len() }> =
    AlignedBytes(*include_bytes!("srs_vesta_accumulator_rkyv.bin"));

const _: () = {
    assert!(core::mem::align_of::<ArchivedRkyvSRS>() <= 16);
    assert!(
        include_bytes!("srs_pallas_kimchi_rkyv.bin").len()
            % core::mem::align_of::<ArchivedRkyvSRS>()
            == 0
    );
    assert!(
        include_bytes!("srs_vesta_accumulator_rkyv.bin").len()
            % core::mem::align_of::<ArchivedRkyvSRS>()
            == 0
    );
};

#[inline(always)]
fn fq_to_bytes<F: CanonicalSerialize>(x: &F) -> [u8; 32] {
    let mut buf = [0u8; 32];
    x.serialize_uncompressed(&mut buf[..])
        .expect("serialize field");
    buf.reverse();
    buf
}

#[inline(always)]
fn flat_to_fp(bytes: &[u8], offset: usize) -> Fp {
    let mut limbs = [0u64; 4];
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes[offset..offset + 32].as_ptr(),
            limbs.as_mut_ptr() as *mut u8,
            32,
        );
        core::mem::transmute(limbs)
    }
}

#[inline(always)]
fn flat_to_fq(bytes: &[u8], offset: usize) -> Fq {
    let mut limbs = [0u64; 4];
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes[offset..offset + 32].as_ptr(),
            limbs.as_mut_ptr() as *mut u8,
            32,
        );
        core::mem::transmute(limbs)
    }
}

#[inline(always)]
fn flat_to_pallas(p: &[u8; 65]) -> Pallas {
    if p[64] != 0 {
        return Pallas::default();
    }
    let x = flat_to_fp(p, 0);
    let y = flat_to_fp(p, 32);
    Pallas::new_unchecked(x, y)
}

#[inline(always)]
fn flat_to_vesta(p: &[u8; 65]) -> Vesta {
    if p[64] != 0 {
        return Vesta::default();
    }
    let x = flat_to_fq(p, 0);
    let y = flat_to_fq(p, 32);
    Vesta::new_unchecked(x, y)
}

fn load_pallas_srs() -> Arc<SRS<Pallas>> {
    let archived = unsafe { rkyv::access_unchecked::<ArchivedRkyvSRS>(&PALLAS_SRS_RKYV.0) };

    let g: Vec<Pallas> = archived.g_flat.iter().map(|p| flat_to_pallas(p)).collect();
    let h = flat_to_pallas(&archived.h_flat);

    let lagrange_bases: Vec<poly_commitment::PolyComm<Pallas>> = archived
        .lagrange_flat
        .iter()
        .map(|p| poly_commitment::PolyComm {
            chunks: vec![flat_to_pallas(p)],
        })
        .collect();

    let mut map = HashMap::new();
    map.insert(
        archived.domain_size.to_native().try_into().unwrap(),
        lagrange_bases,
    );

    Arc::new(SRS::<Pallas> {
        g,
        h,
        lagrange_bases: HashMapCache::new_from_hashmap(map),
    })
}

fn load_vesta_srs() -> Arc<SRS<Vesta>> {
    let archived = unsafe { rkyv::access_unchecked::<ArchivedRkyvSRS>(&VESTA_SRS_RKYV.0) };

    let g: Vec<Vesta> = archived.g_flat.iter().map(|p| flat_to_vesta(p)).collect();
    let h = flat_to_vesta(&archived.h_flat);

    Arc::new(SRS::<Vesta> {
        g,
        h,
        lagrange_bases: HashMapCache::new(),
    })
}

fn commit_zkapp_public_values(
    proof_valid: bool,
    vk_hash: &[u8; 32],
    state_before: &[[u8; 32]; 8],
    state_after: &[[u8; 32]; 8],
    action_state_before: &[u8; 32],
) {
    let mut encoded = Vec::with_capacity(1 + 32 + 8 * 32 + 8 * 32 + 32);
    encoded.push(u8::from(proof_valid));
    encoded.extend_from_slice(vk_hash);

    for field in state_before {
        encoded.extend_from_slice(field);
    }

    for field in state_after {
        encoded.extend_from_slice(field);
    }

    encoded.extend_from_slice(action_state_before);
    debug_assert_eq!(encoded.len(), 577);

    sp1_zkvm::io::commit_slice(&encoded);
}

fn main() {
    // ------------------------------------------------------------------
    // 1. Read inputs
    // ------------------------------------------------------------------
    let vk_wire: MinaBaseVerificationKeyWireStableV1 = sp1_zkvm::io::read();
    let proof: PicklesProofProofsVerified2ReprStableV2 = sp1_zkvm::io::read();
    let zkapp_stmt_raw = sp1_zkvm::io::read_vec();
    let zkapp_cmd_raw = sp1_zkvm::io::read_vec();

    // ------------------------------------------------------------------
    // 2. Deserialize inputs
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: deserialize_inputs");

    let zkapp_stmt: ZkappStatement =
        bincode::deserialize(&zkapp_stmt_raw).expect("deserialize zkapp_stmt");

    let zkapp_cmd_wire: MinaBaseZkappCommandTStableV1WireStableV1 =
        bincode::deserialize(&zkapp_cmd_raw).expect("deserialize zkapp_command wire");

    let zkapp_cmd: ZkAppCommand = (&zkapp_cmd_wire).try_into().expect("wire -> ZkAppCommand");

    println!("cycle-tracker-end: deserialize_inputs");

    // ------------------------------------------------------------------
    // 3. Bind the command used for state extraction to the proven statement
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: verify_account_update_binding");

    assert!(
        !zkapp_cmd.account_updates.0.is_empty(),
        "empty account_updates"
    );

    let first_update = &zkapp_cmd.account_updates.0[0].elt.account_update;
    let recomputed_digest = first_update.digest();

    assert_eq!(
        recomputed_digest, *zkapp_stmt.account_update,
        "zkapp_stmt/account_update mismatch"
    );

    assert_eq!(
        zkapp_cmd.account_updates.0[0].elt.calls.hash(),
        *zkapp_stmt.calls,
        "zkapp_stmt/calls mismatch"
    );

    println!("cycle-tracker-end: verify_account_update_binding");

    // ------------------------------------------------------------------
    // 4. Build the verifier index from the verification key and static SRS
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: build_verifier_index");

    let vk: VerificationKey = (&vk_wire).try_into().expect("vk wire -> runtime");
    let pallas_srs = load_pallas_srs();
    let domains = wrap_domains(vk.actual_wrap_domain_size.to_int());
    let domain =
        Radix2EvaluationDomain::<Fq>::new(domains.h.size() as usize).expect("create wrap domain");
    let verifier_index = make_zkapp_verifier_index_with_srs(&vk, domain, pallas_srs);
    let vk_hash = vk.hash();
    let verifier_index_hash = fq_to_bytes(&vk_hash);

    let make_poly = |poly: &InnerCurve<Fp>| poly_commitment::PolyComm {
        chunks: vec![poly.to_affine()],
    };

    assert_eq!(
        verifier_index.generic_comm,
        make_poly(&vk.wrap_index.generic),
        "generic commitment mismatch"
    );

    assert_eq!(
        verifier_index.sigma_comm,
        vk.wrap_index.sigma.each_ref().map(make_poly),
        "sigma commitments mismatch"
    );

    println!("cycle-tracker-end: build_verifier_index");

    // ------------------------------------------------------------------
    // 5. Verify Pickles accumulator and deferred-value checks
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: pickles_accumulator_check");
    let vesta_srs = load_vesta_srs();
    let accumulator_ok =
        accumulator_check(&vesta_srs, &[&proof]).expect("Pickles accumulator check");
    println!("cycle-tracker-end: pickles_accumulator_check");
    assert!(accumulator_ok, "Pickles accumulator check failed");

    println!("cycle-tracker-start: compute_deferred_values");
    let deferred_values = compute_deferred_values(&proof).expect("compute deferred values");
    let pickles_checks_ok = run_checks(&proof, &verifier_index);
    println!("cycle-tracker-end: compute_deferred_values");
    assert!(pickles_checks_ok, "Pickles run_checks failed");

    // ------------------------------------------------------------------
    // 6. Compute public inputs
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: compute_public_inputs");

    let vk_wrapper = VK {
        commitments: *vk.wrap_index.clone(),
        index: &verifier_index,
        data: (),
    };

    let msg_next_step = get_message_for_next_step_proof(
        &proof.statement.messages_for_next_step_proof,
        &vk_wrapper.commitments,
        &zkapp_stmt,
    )
    .expect("get_message_for_next_step_proof");

    let msg_next_wrap =
        get_message_for_next_wrap_proof(&proof.statement.proof_state.messages_for_next_wrap_proof)
            .expect("get_message_for_next_wrap_proof");

    let prepared = get_prepared_statement(
        &msg_next_step,
        &msg_next_wrap,
        deferred_values,
        &proof.statement.proof_state.sponge_digest_before_evaluations,
    );

    let public_inputs: Vec<Fq> = prepared
        .to_public_input(vk_wrapper.index.public)
        .expect("prepared -> public inputs");

    println!("cycle-tracker-end: compute_public_inputs");

    // ------------------------------------------------------------------
    // 7. Pad proof + group map
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: make_padded_proof");
    let prover_proof = make_padded_proof_from_p2p(&proof).expect("padded proof");
    println!("cycle-tracker-end: make_padded_proof");

    println!("cycle-tracker-start: group_map_setup");
    let group_map = GroupMap::<Fp>::setup();
    println!("cycle-tracker-end: group_map_setup");

    // ------------------------------------------------------------------
    // 8. Verify the outer Kimchi proof
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: kimchi_verify");

    let result = kimchi::verifier::verify::<
        FULL_ROUNDS,
        Pallas,
        EFqSponge,
        EFrSponge,
        OpeningProof<Pallas, FULL_ROUNDS>,
    >(&group_map, &verifier_index, &prover_proof, &public_inputs);

    println!("cycle-tracker-end: kimchi_verify");

    let proof_valid = accumulator_ok && pickles_checks_ok && result.is_ok();
    assert!(proof_valid, "Pickles verify failed: {:?}", result.err());

    // ------------------------------------------------------------------
    // 9. Resolve canonical state transition
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: resolve_settlement_values");

    let body = &first_update.body;

    let mut app_state_before = [[0u8; 32]; 8];
    let mut app_state_after = [[0u8; 32]; 8];

    for i in 0..8 {
        app_state_before[i] = match &body.preconditions.account.0.state[i] {
            OrIgnore::Check(f) => fq_to_bytes(f),
            OrIgnore::Ignore => [0u8; 32],
        };

        app_state_after[i] = match &body.update.app_state[i] {
            SetOrKeep::Set(f) => fq_to_bytes(f),
            SetOrKeep::Keep => [0u8; 32],
        };
    }

    let action_state_before = match &body.preconditions.account.0.action_state {
        OrIgnore::Check(x) => fq_to_bytes(x),
        OrIgnore::Ignore => [0u8; 32],
    };

    println!("cycle-tracker-end: resolve_settlement_values");

    // ------------------------------------------------------------------
    // 10. Commit settlement outputs
    // ------------------------------------------------------------------
    commit_zkapp_public_values(
        proof_valid,
        &verifier_index_hash,
        &app_state_before,
        &app_state_after,
        &action_state_before,
    );

    sp1_zkvm::syscalls::syscall_halt(0);
}
