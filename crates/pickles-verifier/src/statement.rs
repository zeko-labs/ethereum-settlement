//! Binding between the application statement supplied by the host and the
//! message digest verified by the Pickles wrap proof.
//!
//! The host-side OCaml wire conversion has to reconstruct this digest because
//! the serialized proof skeleton erases it. Recomputing it again in the guest
//! prevents a malicious host from pairing a valid proof with unrelated
//! application-statement fields.

use alloc::vec::Vec;

use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::pasta::{fp_kimchi, FULL_ROUNDS};
use mina_poseidon::poseidon::{ArithmeticSponge, Sponge};

use crate::types::{StepField, VerifiableProof, Verifier, STEP_IPA_ROUNDS};

fn wrap_vk_step_fields(verifier: &Verifier) -> Option<Vec<StepField>> {
    let vk = &verifier.wrap_vk;
    let index_comms = [
        &vk.generic_comm,
        &vk.psm_comm,
        &vk.complete_add_comm,
        &vk.mul_comm,
        &vk.emul_comm,
        &vk.endomul_scalar_comm,
    ];

    vk.sigma_comm
        .iter()
        .chain(vk.coefficients_comm.iter())
        .chain(index_comms)
        .try_fold(Vec::with_capacity((7 + 15 + 6) * 2), |mut out, pc| {
            let point = pc.chunks.first()?;
            out.push(point.x);
            out.push(point.y);
            Some(out)
        })
}

/// Recompute `Common.hash_messages_for_next_step_proof` from the application
/// statement and previous-proof commitments carried in [`VerifiableProof`].
pub fn app_state_digest(verifier: &Verifier, proof: &VerifiableProof) -> Option<StepField> {
    if proof.prev_step_sgs.len() != proof.old_bulletproof_challenges.len() {
        return None;
    }

    let vk_fields = wrap_vk_step_fields(verifier)?;
    let mut inputs = Vec::with_capacity(
        vk_fields.len() + proof.app_state.len() + proof.prev_step_sgs.len() * (2 + STEP_IPA_ROUNDS),
    );
    inputs.extend_from_slice(&vk_fields);
    inputs.extend_from_slice(&proof.app_state);
    for (sg, challenges) in proof
        .prev_step_sgs
        .iter()
        .zip(proof.old_bulletproof_challenges.iter())
    {
        inputs.push(sg.x);
        inputs.push(sg.y);
        inputs.extend_from_slice(challenges);
    }

    let mut sponge: ArithmeticSponge<StepField, PlonkSpongeConstantsKimchi, FULL_ROUNDS> =
        Sponge::new(fp_kimchi::static_params());
    sponge.absorb(&inputs);
    Some(sponge.squeeze())
}

pub fn check_app_state_binding(verifier: &Verifier, proof: &VerifiableProof) -> bool {
    app_state_digest(verifier, proof) == Some(proof.messages_for_next_step_proof_digest)
}
