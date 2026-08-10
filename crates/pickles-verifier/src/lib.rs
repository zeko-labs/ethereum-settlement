//! Out-of-circuit Pickles verifier.
//!
//! Layering:
//!
//!   * [`wire`] (std) — serde-deserializable form of the four OCaml-produced
//!     fixture files, plus parsing. Host-side only.
//!   * [`convert`] (std) — host-side conversion of the parsed wire data into
//!     the verifier types ([`Verifier::new`], [`OcamlProof::into_verifiable`]).
//!   * [`types`] (no_std) — the verifier types ([`Verifier`],
//!     [`VerifiableProof`]) consumed by the crate-root [`verify`] /
//!     [`verify_batch`] entry points. These run `no_std` + `alloc`, suitable
//!     for an SP1 guest.
//!
//! Primitives (Pasta fields/curves, Poseidon, kimchi `VerifierIndex` /
//! `ProverProof` serde, SRS/MSM, `batch_verify_with_rng`, the linearization
//! interpreter) come from upstream proof-systems crates; only the
//! pickles-specific glue is implemented here.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod deferred;
pub mod serialize;
pub mod statement;
pub mod types;

#[cfg(feature = "std")]
pub mod wire;

#[cfg(feature = "std")]
pub mod convert;

use alloc::vec::Vec;

use ark_ec::{AffineRepr, CurveGroup, VariableBaseMSM};
use mina_curves::pasta::Vesta;
use mina_poseidon::sponge::ScalarChallenge;
use poly_commitment::commitment::b_poly_coefficients;

use types::{StepField, VerifiableProof, Verifier};

#[cfg(target_os = "zkvm")]
fn track_cycles(command: &[u8]) {
    sp1_lib::io::write(1, command);
}

#[cfg(not(target_os = "zkvm"))]
fn track_cycles(_command: &[u8]) {}

/// Verify a single Pickles proof against its tag's [`Verifier`]. Deterministic
/// + `no_std`.
pub fn verify(verifier: &Verifier, proof: &VerifiableProof) -> bool {
    verify_batch(verifier, core::slice::from_ref(proof))
}

/// Verify a batch of proofs sharing one tag. Deterministic + `no_std`.
///
/// Three stages, AND-folded:
///   1. [`accumulator_check`] per proof.
///   2. Reconstruct each proof's wrap kimchi public input from its expanded
///      deferred values ([`deferred::expand_deferred`] +
///      [`deferred::wrap_public_input`]).
///   3. One amortized kimchi `batch_verify_with_rng` over the batch, seeded by
///      [`batching_rng`] (Fiat–Shamir-derived; see its doc for the soundness
///      argument).
pub fn verify_batch(verifier: &Verifier, proofs: &[VerifiableProof]) -> bool {
    use groupmap::GroupMap;
    use kimchi::verifier::{batch_verify_with_rng, Context};
    use mina_curves::pasta::{Pallas, PallasParameters};
    use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
    use mina_poseidon::pasta::FULL_ROUNDS;
    use mina_poseidon::sponge::{DefaultFqSponge, DefaultFrSponge};
    use poly_commitment::commitment::CommitmentCurve;
    use poly_commitment::ipa::OpeningProof;
    use rand_chacha::ChaCha20Rng;
    use types::WrapField;

    if !proofs
        .iter()
        .all(|p| statement::check_app_state_binding(verifier, p) && accumulator_check(verifier, p))
    {
        return false;
    }

    track_cycles(b"cycle-tracker-report-start:pickles-wrap-prepare\n");
    let pis: Vec<Vec<WrapField>> = proofs
        .iter()
        .map(|p| {
            let dv = deferred::expand_deferred(verifier, p);
            deferred::wrap_public_input(
                &dv,
                p.messages_for_next_step_proof_digest,
                p.messages_for_next_wrap_proof_digest,
            )
        })
        .collect();

    let mut rng = batching_rng(proofs, &pis);
    let contexts: Vec<Context<FULL_ROUNDS, Pallas, OpeningProof<Pallas, FULL_ROUNDS>, _>> = proofs
        .iter()
        .zip(pis.iter())
        .map(|(p, pi)| Context {
            verifier_index: &verifier.wrap_vk,
            proof: &p.wrap_proof,
            public_input: pi.as_slice(),
        })
        .collect();
    track_cycles(b"cycle-tracker-report-end:pickles-wrap-prepare\n");

    let group_map = <Pallas as CommitmentCurve>::Map::setup();
    type WrapFqSponge = DefaultFqSponge<PallasParameters, PlonkSpongeConstantsKimchi, FULL_ROUNDS>;
    type WrapFrSponge = DefaultFrSponge<WrapField, PlonkSpongeConstantsKimchi, FULL_ROUNDS>;
    track_cycles(b"cycle-tracker-report-start:pickles-wrap-kimchi\n");
    let verified = batch_verify_with_rng::<
        FULL_ROUNDS,
        Pallas,
        WrapFqSponge,
        WrapFrSponge,
        OpeningProof<Pallas, FULL_ROUNDS>,
        ChaCha20Rng,
    >(&group_map, &contexts, &mut rng)
    .is_ok();
    track_cycles(b"cycle-tracker-report-end:pickles-wrap-kimchi\n");
    verified
}

/// Fiat–Shamir-derived `ChaCha20Rng` for kimchi's batched dlog check, seeded
/// by `blake2s(domain ‖ for each (proof, pi): postcard(proof) ‖ canonical(pi))`
/// with length prefixes.
///
/// Binding the random linear combination of the IPA verification equations to
/// the proof is what makes the batched check sound: a fixed/known seed would
/// let a prover forge an invalid proof whose specific combination vanishes.
/// `postcard` is used for the (serde-only) `ProverProof`; the public input
/// uses `CanonicalSerialize` (bare `Vec<Fq>` doesn't impl native serde
/// `Serialize`, only the proof's `serde_as` wrappers do).
fn batching_rng(
    proofs: &[VerifiableProof],
    pis: &[Vec<types::WrapField>],
) -> rand_chacha::ChaCha20Rng {
    use ark_serialize::CanonicalSerialize;
    use blake2::{Blake2s256, Digest};
    use rand_chacha::ChaCha20Rng;
    use rand_core::SeedableRng;

    let mut hasher = Blake2s256::new();
    hasher.update(b"pickles-verifier/batch-dlog-rng/v1");
    for (p, pi) in proofs.iter().zip(pis.iter()) {
        let proof_bytes = postcard::to_allocvec(&p.wrap_proof).expect("serialize wrap proof");
        hasher.update((proof_bytes.len() as u64).to_le_bytes());
        hasher.update(&proof_bytes);
        let mut pi_bytes = Vec::new();
        pi.serialize_compressed(&mut pi_bytes)
            .expect("serialize public input");
        hasher.update((pi_bytes.len() as u64).to_le_bytes());
        hasher.update(&pi_bytes);
    }
    ChaCha20Rng::from_seed(hasher.finalize().into())
}

/// Stage 2: the IPA accumulator check. The proof's
/// `challenge_polynomial_commitment` must equal `compute_sg(bulletproof_challenges)`,
/// the non-hiding MSM of the IPA challenge polynomial `b(X)`'s coefficients
/// against the Vesta SRS generators. Plain arkworks MSM rather than the
/// std-gated `SRS::commit_non_hiding`.
pub fn accumulator_check(verifier: &Verifier, proof: &VerifiableProof) -> bool {
    track_cycles(b"cycle-tracker-report-start:pickles-accumulator\n");
    let chals: Vec<StepField> = proof
        .raw_bulletproof_challenges
        .iter()
        .map(|c| ScalarChallenge::new(*c).to_field(&verifier.step_endo))
        .collect();
    let coeffs = b_poly_coefficients(&chals);
    let g = &verifier.vesta_srs.g;
    let expected_len = 1usize << verifier.step_srs_length_log2;
    if coeffs.len() != expected_len || g.len() < expected_len {
        track_cycles(b"cycle-tracker-report-end:pickles-accumulator\n");
        return false;
    }
    let computed_sg =
        <<Vesta as AffineRepr>::Group as VariableBaseMSM>::msm(&g[..expected_len], &coeffs)
            .expect("compute_sg MSM")
            .into_affine();
    let valid = computed_sg == proof.challenge_polynomial_commitment;
    track_cycles(b"cycle-tracker-report-end:pickles-accumulator\n");
    valid
}

// The tests use `std::fs` to read fixture JSONs + `std::sync::OnceLock` for
// the shared SRSes, so they only build under `--features std`. The no_std
// build covers serialize's pod-layout tests (`mod serialize::tests`) and the
// wire/convert sibling modules' tests already gated by `cfg(feature = "std")`.
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::types::{VestaSrs, WrapSrs, STEP_IPA_ROUNDS};
    use crate::wire::{
        parse_app_statement, parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk,
        OcamlProof,
    };
    use ark_ff::{AdditiveGroup, Field, PrimeField, Zero};
    use mina_curves::pasta::{Pallas, Vesta};
    use poly_commitment::precomputed_srs::get_srs;
    use std::sync::{Arc, OnceLock};

    /// Shared Vesta SRS for the stage-2 accumulator MSM (loaded once from
    /// `proof-systems/srs/vesta.srs`).
    fn vesta_srs() -> &'static Arc<VestaSrs> {
        static SRS: OnceLock<Arc<VestaSrs>> = OnceLock::new();
        SRS.get_or_init(|| Arc::new(get_srs::<Vesta>()))
    }

    /// Shared Pallas SRS for the wrap dlog check (loaded once from
    /// `proof-systems/srs/pallas.srs`).
    fn wrap_srs() -> &'static Arc<WrapSrs> {
        static SRS: OnceLock<Arc<WrapSrs>> = OnceLock::new();
        SRS.get_or_init(|| Arc::new(get_srs::<Pallas>()))
    }

    fn fixture(dir: &str, file: &str) -> String {
        let path = format!("{}/../../fixtures/{dir}/{file}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
    }

    /// Full end-to-end out-of-circuit verification over the fixture matrix.
    /// A `Verifier` is built per fixture (each has its own wrap VK); SRSes are
    /// shared across iterations via `OnceLock`.
    #[test]
    fn verify_accepts_fixtures() {
        for dir in [
            "mainnet-blockchain-snark",
            "zeko-local-e2e",
            "nrr",
            "simplechain/wrap0",
            "simplechain/wrap1",
            "simplechain/wrap2",
            "treeproofreturn/wrap0",
            "treeproofreturn/wrap1",
            "treeproofreturn/wrap2",
        ] {
            let ocaml =
                OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
            let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
            let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
            let stmt =
                parse_app_statement_fields(&fixture(dir, "app_statement.json")).expect("stmt");

            let vp = ocaml
                .into_verifiable(wrap_proof, &wrap_vk, &stmt)
                .expect("conversion");
            let verifier = Verifier::new(wrap_vk, wrap_srs().clone(), vesta_srs().clone(), 1);

            assert!(verify(&verifier, &vp), "verify should accept {dir}");
        }
    }

    #[test]
    fn verify_rejects_mutated_application_statement() {
        use ark_ff::One;

        let dir = "mainnet-blockchain-snark";
        let ocaml =
            OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
        let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
        let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
        let stmt = parse_app_statement(&fixture(dir, "app_statement.json")).expect("stmt");
        let mut proof = ocaml
            .into_verifiable(wrap_proof, &wrap_vk, &[stmt])
            .expect("conversion");
        let verifier = Verifier::new(wrap_vk, wrap_srs().clone(), vesta_srs().clone(), 1);

        proof.app_state[0] += StepField::one();

        assert!(
            !verify(&verifier, &proof),
            "application statement must be bound inside the verifier"
        );
    }

    fn mainnet_proof() -> (Verifier, VerifiableProof) {
        let dir = "mainnet-blockchain-snark";
        let ocaml =
            OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
        let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
        let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
        let stmt = parse_app_statement(&fixture(dir, "app_statement.json")).expect("stmt");
        let proof = ocaml
            .into_verifiable(wrap_proof, &wrap_vk, &[stmt])
            .expect("conversion");
        let verifier = Verifier::new(wrap_vk, wrap_srs().clone(), vesta_srs().clone(), 1);
        (verifier, proof)
    }

    fn zeko_proof() -> (Verifier, VerifiableProof) {
        let dir = "zeko-local-e2e";
        let ocaml =
            OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
        let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
        let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
        let stmt = parse_app_statement_fields(&fixture(dir, "app_statement.json"))
            .expect("application statement");
        let proof = ocaml
            .into_verifiable(wrap_proof, &wrap_vk, &stmt)
            .expect("conversion");
        let verifier = Verifier::new(wrap_vk, wrap_srs().clone(), vesta_srs().clone(), 1);
        (verifier, proof)
    }

    fn assert_serial_msm_matches_naive<C>(len: usize)
    where
        C: AffineRepr,
        C::Group: VariableBaseMSM<MulBase = C>,
        C::ScalarField: PrimeField,
    {
        let bases: Vec<C> = (0..len)
            .map(|i| (C::generator() * C::ScalarField::from((i as u64) + 2)).into_affine())
            .collect();
        let scalars: Vec<C::ScalarField> = (0..len)
            .map(|i| {
                let mut scalar = C::ScalarField::from((i as u64) + 11);
                for round in 0..4 {
                    scalar.square_in_place();
                    scalar += C::ScalarField::from((i + round + 3) as u64);
                }
                match i % 19 {
                    0 => C::ScalarField::ZERO,
                    1 => C::ScalarField::ONE,
                    _ => scalar,
                }
            })
            .collect();
        let bigints: Vec<_> = scalars.iter().map(|scalar| scalar.into_bigint()).collect();

        let serial =
            ark_ec::scalar_mul::variable_base::msm_bigint_serial::<C::Group>(&bases, &bigints);
        let naive = bases
            .iter()
            .zip(&scalars)
            .fold(C::Group::zero(), |mut sum, (base, scalar)| {
                sum += *base * scalar;
                sum
            });

        assert_eq!(serial, naive, "serial MSM mismatch at length {len}");
    }

    #[test]
    fn zkvm_serial_msm_matches_naive_on_both_pasta_curves() {
        for len in [0, 1, 2, 31, 32, 257] {
            assert_serial_msm_matches_naive::<Pallas>(len);
            assert_serial_msm_matches_naive::<Vesta>(len);
        }
    }

    #[test]
    fn zkvm_serial_msm_accepts_the_real_accumulator_commitment() {
        let (verifier, proof) = mainnet_proof();
        let challenges: Vec<StepField> = proof
            .raw_bulletproof_challenges
            .iter()
            .map(|challenge| ScalarChallenge::new(*challenge).to_field(&verifier.step_endo))
            .collect();
        let coefficients = b_poly_coefficients(&challenges);
        assert_eq!(coefficients.len(), 1usize << STEP_IPA_ROUNDS);
        assert!(verifier.vesta_srs.g.len() >= coefficients.len());
        let bigints: Vec<_> = coefficients
            .iter()
            .map(|coefficient| coefficient.into_bigint())
            .collect();

        let commitment = ark_ec::scalar_mul::variable_base::msm_bigint_serial::<
            <Vesta as AffineRepr>::Group,
        >(&verifier.vesta_srs.g[..coefficients.len()], &bigints)
        .into_affine();

        assert_eq!(commitment, proof.challenge_polynomial_commitment);
    }

    #[test]
    fn accumulator_rejects_a_truncated_step_srs() {
        let (mut verifier, proof) = mainnet_proof();
        let mut truncated = VestaSrs::default();
        truncated.g = verifier.vesta_srs.g[..(1usize << STEP_IPA_ROUNDS) - 1].to_vec();
        verifier.vesta_srs = Arc::new(truncated);

        assert!(!accumulator_check(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_deferred_plonk_challenge() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.raw_plonk.alpha += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_bulletproof_challenge() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.raw_bulletproof_challenges[0] += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_accumulator_commitment() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.challenge_polynomial_commitment.x += types::WrapField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_previous_proof_challenge() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.old_bulletproof_challenges[0][0] += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_previous_evaluation() {
        use ark_ff::One;
        let (verifier, mut proof) = mainnet_proof();
        proof.prev_evals.ft_eval1 += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_feature_flag() {
        let (verifier, mut proof) = zeko_proof();
        proof.raw_plonk.feature_flags.range_check0 = false;
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_joint_combiner() {
        use ark_ff::One;
        let (verifier, mut proof) = zeko_proof();
        *proof
            .raw_plonk
            .joint_combiner
            .as_mut()
            .expect("Zeko uses lookup parameters") += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    #[test]
    fn verify_rejects_mutated_lookup_evaluation() {
        use ark_ff::One;
        let (verifier, mut proof) = zeko_proof();
        proof
            .prev_evals
            .lookup_aggregation
            .as_mut()
            .expect("Zeko uses lookup aggregation")
            .zeta[0] += StepField::one();
        assert!(!verify(&verifier, &proof));
    }

    /// Encode→decode round-trip: build a [`Verifier`] from the mainnet
    /// blockchain SNARK fixture, run it through
    /// [`serialize::encode_verifier_blob`] then [`serialize::decode_verifier_blob`],
    /// and confirm the decoded verifier (with a pod-cast SRS + pre-seeded
    /// wrap Lagrange basis) accepts the same proof. This is the path the SP1
    /// guest exercises.
    #[test]
    fn encode_decode_verifier_blob_round_trip_accepts() {
        let dir = "mainnet-blockchain-snark";
        let ocaml =
            OcamlProof::parse(&fixture(dir, "public_input_skeleton.json")).expect("skeleton");
        let wrap_vk = parse_wrap_vk(&fixture(dir, "vk.serde.json")).expect("vk");
        let wrap_proof = parse_wrap_proof(&fixture(dir, "proof.serde.json")).expect("proof");
        let stmt = parse_app_statement(&fixture(dir, "app_statement.json")).expect("stmt");
        let vp = ocaml
            .into_verifiable(wrap_proof, &wrap_vk, &[stmt])
            .expect("conversion");

        // Encode at host side (basis is computed from the wrap SRS at wrap_vk's
        // domain), then decode (basis is seeded into the new SRS's cache).
        let blob = crate::serialize::encode_verifier_blob(
            vesta_srs(),
            wrap_srs(),
            /* step_num_chunks */ 1,
            &wrap_vk,
        );
        // The decoder needs 8-byte alignment, which `Vec<u8>` already provides
        // on this platform (the underlying allocator returns max-aligned
        // blocks). The SP1 guest gets it from a `#[repr(C, align(8))]` wrapper
        // around `include_bytes!`.
        assert_eq!(blob.as_ptr() as usize % 8, 0, "blob ptr must be 8-aligned");
        let decoded = crate::serialize::decode_verifier_blob(&blob);

        assert!(
            verify(&decoded, &vp),
            "decoded verifier should accept mainnet blockchain SNARK"
        );
    }
}
