#![no_main]
sp1_zkvm::entrypoint!(main);

use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use kimchi::{
    circuits::constraints::FeatureFlags, groupmap::GroupMap, linearization::expr_linearization,
    mina_curves::pasta::PallasParameters,
};
use ledger::proofs::public_input::plonk_checks::ShiftedValue;
use ledger::proofs::public_input::prepared_statement::{DeferredValues, Plonk};
use ledger::proofs::step::FeatureFlags as LFF;
use ledger::proofs::transaction::{endos, InnerCurve};
use ledger::proofs::verification::{
    get_message_for_next_step_proof, get_message_for_next_wrap_proof, get_prepared_statement, VK,
};
use ledger::proofs::{prover::make_padded_proof_from_p2p, VerifierIndex};
use ledger::scan_state::transaction_logic::zkapp_command::{OrIgnore, SetOrKeep, ZkAppCommand};
use ledger::scan_state::transaction_logic::zkapp_statement::ZkappStatement;
use ledger::VerificationKey;
use mina_curves::pasta::{Fp, Fq, Pallas};
use mina_p2p_messages::v2::{
    CompositionTypesBranchDataDomainLog2StableV1, CompositionTypesBranchDataStableV1,
    MinaBaseVerificationKeyWireStableV1, MinaBaseZkappCommandTStableV1WireStableV1,
    PicklesBaseProofsVerifiedStableV1, PicklesProofProofsVerified2ReprStableV2,
};
use mina_poseidon::sponge::{DefaultFqSponge, DefaultFrSponge};
use poly_commitment::{
    hash_map_cache::HashMapCache,
    ipa::{OpeningProof, SRS},
};
use std::{collections::HashMap, sync::Arc};
use zeko_sp1_lib::{ArchivedRkyvSRS, SerializableDeferredValues, ZkappPublicValues};

const FULL_ROUNDS: usize = 55;
type SpongeParams = mina_poseidon::constants::PlonkSpongeConstantsKimchi;
type EFqSponge = DefaultFqSponge<PallasParameters, SpongeParams, FULL_ROUNDS>;
type EFrSponge = DefaultFrSponge<Fq, SpongeParams, FULL_ROUNDS>;

#[repr(align(16))]
struct AlignedBytes<const N: usize>([u8; N]);

static SRS_RKYV: AlignedBytes<{ include_bytes!("srs_rkyv.bin").len() }> =
    AlignedBytes(*include_bytes!("srs_rkyv.bin"));

const _: () = {
    assert!(core::mem::align_of::<ArchivedRkyvSRS>() <= 16);
    assert!(include_bytes!("srs_rkyv.bin").len() % core::mem::align_of::<ArchivedRkyvSRS>() == 0);
};

// ---------- Helpers ----------

#[inline(always)]
fn deserialize_fp(bytes: [u8; 32]) -> Fp {
    Fp::deserialize_uncompressed(&bytes[..]).expect("invalid Fp encoding")
}

#[inline(always)]
fn fq_to_bytes<F: CanonicalSerialize>(x: &F) -> [u8; 32] {
    let mut buf = [0u8; 32];
    x.serialize_uncompressed(&mut buf[..])
        .expect("serialize field");
    buf.reverse(); // ark-serialize outputs little-endian, EVM expects big-endian
    buf
}

#[inline(always)]
fn to_shifted_value(bytes: [u8; 32]) -> ShiftedValue<Fp> {
    ShiftedValue {
        shifted: deserialize_fp(bytes),
    }
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
fn flat_to_pallas(p: &[u8; 65]) -> Pallas {
    if p[64] != 0 {
        return Pallas::default();
    }
    let x = flat_to_fp(p, 0);
    let y = flat_to_fp(p, 32);
    Pallas::new_unchecked(x, y)
}

fn main() {
    // ------------------------------------------------------------------
    // 1. Read inputs
    // ------------------------------------------------------------------
    let vk_wire: MinaBaseVerificationKeyWireStableV1 = sp1_zkvm::io::read();
    let proof: PicklesProofProofsVerified2ReprStableV2 = sp1_zkvm::io::read();
    let zkapp_stmt_raw = sp1_zkvm::io::read_vec();
    let deferred_values_raw = sp1_zkvm::io::read_vec();
    let zkapp_cmd_raw = sp1_zkvm::io::read_vec();
    let verifier_index_raw = sp1_zkvm::io::read_vec();

    // ------------------------------------------------------------------
    // 2. Deserialize inputs
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: deserialize_inputs");

    let zkapp_stmt: ZkappStatement =
        bincode::deserialize(&zkapp_stmt_raw).expect("deserialize zkapp_stmt");

    let sdv: SerializableDeferredValues =
        bincode::deserialize(&deferred_values_raw).expect("deserialize deferred_values");

    let deferred_values = DeferredValues {
        plonk: Plonk {
            alpha: sdv.plonk.alpha,
            beta: sdv.plonk.beta,
            gamma: sdv.plonk.gamma,
            zeta: sdv.plonk.zeta,
            zeta_to_srs_length: to_shifted_value(sdv.plonk.zeta_to_srs_length),
            zeta_to_domain_size: to_shifted_value(sdv.plonk.zeta_to_domain_size),
            perm: to_shifted_value(sdv.plonk.perm),
            lookup: sdv.plonk.lookup,
            feature_flags: LFF {
                range_check0: sdv.plonk.feature_flags_range_check0,
                range_check1: sdv.plonk.feature_flags_range_check1,
                foreign_field_add: sdv.plonk.feature_flags_foreign_field_add,
                foreign_field_mul: sdv.plonk.feature_flags_foreign_field_mul,
                xor: sdv.plonk.feature_flags_xor,
                rot: sdv.plonk.feature_flags_rot,
                lookup: sdv.plonk.feature_flags_lookup,
                runtime_tables: sdv.plonk.feature_flags_runtime_tables,
            },
        },
        combined_inner_product: to_shifted_value(sdv.combined_inner_product),
        b: to_shifted_value(sdv.b),
        xi: sdv.xi,
        bulletproof_challenges: sdv
            .bulletproof_challenges
            .iter()
            .map(|b| deserialize_fp(*b))
            .collect(),
        branch_data: CompositionTypesBranchDataStableV1 {
            proofs_verified: match sdv.branch_data_proofs_verified {
                0 => PicklesBaseProofsVerifiedStableV1::N0,
                1 => PicklesBaseProofsVerifiedStableV1::N1,
                _ => PicklesBaseProofsVerifiedStableV1::N2,
            },
            domain_log2: CompositionTypesBranchDataDomainLog2StableV1(
                sdv.branch_data_domain_log2.into(),
            ),
        },
    };

    let zkapp_cmd_wire: MinaBaseZkappCommandTStableV1WireStableV1 =
        bincode::deserialize(&zkapp_cmd_raw).expect("deserialize zkapp_command wire");

    let zkapp_cmd: ZkAppCommand = (&zkapp_cmd_wire).try_into().expect("wire -> ZkAppCommand");

    println!("cycle-tracker-end: deserialize_inputs");

    // ------------------------------------------------------------------
    // 3. Bind zkapp_cmd <-> zkapp_stmt
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
    // 4. Deserialize verifier index
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: deserialize_verifier_index");
    let mut verifier_index: VerifierIndex<Fq> =
        bincode::deserialize(&verifier_index_raw).expect("deserialize verifier_index");
    println!("cycle-tracker-end: deserialize_verifier_index");

    // ------------------------------------------------------------------
    // 5. Load static SRS
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: load_static_srs");

    let archived = unsafe { rkyv::access_unchecked::<ArchivedRkyvSRS>(&SRS_RKYV.0) };

    let g: Vec<Pallas> = archived.g_flat.iter().map(|p| flat_to_pallas(p)).collect();

    let h: Pallas = flat_to_pallas(&archived.h_flat);

    let lagrange_bases: Vec<poly_commitment::PolyComm<Pallas>> = archived
        .lagrange_flat
        .iter()
        .map(|p| poly_commitment::PolyComm {
            chunks: vec![flat_to_pallas(p)],
        })
        .collect();

    let domain_size = archived.domain_size.to_native();

    let mut map = HashMap::new();
    map.insert(domain_size.try_into().unwrap(), lagrange_bases);

    let lagrange_bases = HashMapCache::new_from_hashmap(map);

    let srs = SRS::<Pallas> {
        g,
        h,
        lagrange_bases,
    };
    let srs = Arc::new(srs);

    // The serialized verifier index carries a placeholder SRS. Avoid dropping
    // it inside the guest when installing the real static SRS.
    let old_srs = core::mem::replace(&mut verifier_index.srs, srs);
    core::mem::forget(old_srs);

    println!("cycle-tracker-end: load_static_srs");

    // ------------------------------------------------------------------
    // 6. Reconstruct skipped fields
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: reconstruct_skip_fields");

    let feature_flags = FeatureFlags::default();
    let (linearization, powers_of_alpha) = expr_linearization(Some(&feature_flags), true);
    let (endo_q, _) = endos::<Fq>();

    verifier_index.linearization = linearization;
    verifier_index.powers_of_alpha = powers_of_alpha;
    verifier_index.endo = endo_q;

    println!("cycle-tracker-end: reconstruct_skip_fields");

    // ------------------------------------------------------------------
    // 7. Stronger circuit binding
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: verify_circuit_binding");

    let vk: VerificationKey = (&vk_wire).try_into().expect("vk wire -> runtime");

    let make_poly = |poly: &InnerCurve<Fp>| poly_commitment::PolyComm {
        chunks: vec![poly.to_affine()],
    };

    // These checks are partial but still useful as sanity guards.
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

    let vk_hash = vk.hash();

    let verifier_index_hash = fq_to_bytes(&vk_hash);

    println!("cycle-tracker-end: verify_circuit_binding");

    // ------------------------------------------------------------------
    // 8. Compute public inputs
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
    // 9. Pad proof + group map
    // ------------------------------------------------------------------
    println!("cycle-tracker-start: make_padded_proof");
    let prover_proof = make_padded_proof_from_p2p(&proof).expect("padded proof");
    println!("cycle-tracker-end: make_padded_proof");

    println!("cycle-tracker-start: group_map_setup");
    let group_map = GroupMap::<Fp>::setup();
    println!("cycle-tracker-end: group_map_setup");

    // ------------------------------------------------------------------
    // 10. Verify Kimchi proof
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

    let proof_valid = result.is_ok();
    assert!(proof_valid, "Kimchi verify failed: {:?}", result.err());

    // ------------------------------------------------------------------
    // 11. Resolve canonical state transition
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

        // app_state_after[i] = match &body.update.app_state[i] {
        //     SetOrKeep::Set(f) => fq_to_bytes(f),
        //     SetOrKeep::Keep => match &body.preconditions.account.0.state[i] {
        //         OrIgnore::Check(f) => fq_to_bytes(f),
        //         OrIgnore::Ignore => [0u8; 32],
        //     },
        // };
        app_state_after[i] = match &body.update.app_state[i] {
            SetOrKeep::Set(f) => fq_to_bytes(f),
            SetOrKeep::Keep => [0u8; 32],
        };
    }

    // Same caveat: this assumes the canonical rollup transition is encoded here.
    // Adjust these paths to your actual rollup semantics.
    let action_state_before = match &body.preconditions.account.0.action_state {
        OrIgnore::Check(x) => fq_to_bytes(x),
        OrIgnore::Ignore => [0u8; 32],
    };

    println!("cycle-tracker-end: resolve_settlement_values");

    // ------------------------------------------------------------------
    // 12. Commit settlement outputs
    // ------------------------------------------------------------------

    sp1_zkvm::io::commit(&ZkappPublicValues {
        proof_valid,
        vk_hash: verifier_index_hash,
        state_before: app_state_before,
        state_after: app_state_after,
        action_state_before,
    });
}
