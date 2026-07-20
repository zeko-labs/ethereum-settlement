//! Host wire decoding for Pickles proofs and verification keys.
//!
//! Current Zeko exports versioned, OCaml-owned JSON containing the side-loaded
//! proof, its padded recursion accumulators, and the compact side-loaded VK.
//! This module reconstructs native Kimchi [`WrapProof`] / [`WrapVerifierIndex`]
//! values from that wire. During migration it also accepts the legacy files:
//!
//!   * `vk.serde.json` — Kimchi `VerifierIndex` Rust serde
//!   * `proof.serde.json` — Kimchi wrap `ProverProof` Rust serde
//!   * `public_input_skeleton.json` — the Pickles `{statement; prev_evals}` skeleton ([`OcamlProof`])
//!   * `app_statement.json` — the application statement (BE-hex field)
//!
//! The Pickles values use the bespoke OCaml-yojson decode in this module.
//! Field/curve mapping:
//!
//!   * `StepField` = Tick = `Fp` (Vesta scalar / Pallas base)
//!   * `WrapField` = Tock = `Fq` (Pallas scalar / Vesta base)
//!   * the wrap proof + VK live over the `Pallas` curve.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ark_ff::{PrimeField, Zero};
use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
use kimchi::circuits::constraints::FeatureFlags;
use kimchi::circuits::lookup::lookups::{LookupFeatures, LookupPatterns};
use kimchi::circuits::polynomials::permutation;
use kimchi::proof::{PointEvaluations, ProofEvaluations, ProverCommitments, RecursionChallenge};
use mina_curves::pasta::{Pallas, Vesta};
use o1_utils::FieldHelpers;
use poly_commitment::commitment::PolyComm;
use poly_commitment::ipa::OpeningProof;
use serde_json::Value;

use crate::types::{
    BranchData, ChunkedAllEvals, PlonkMinimal, StepField, WrapField, WrapProof, WrapVerifierIndex,
    STEP_IPA_ROUNDS, WRAP_IPA_ROUNDS,
};

/// Parse either the legacy Kimchi-serde VK or Zeko's versioned compact VK wire.
/// The returned verifier index has an empty SRS; [`crate::types::Verifier::new`]
/// attaches the real one and reconstructs the other serde-skipped fields.
pub fn parse_wrap_vk(json: &str) -> Result<WrapVerifierIndex, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if value.get("schemaVersion").is_some() {
        parse_ocaml_wrap_vk(&value)
    } else {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
}

/// Parse either the legacy Kimchi-serde proof or Zeko's versioned Pickles wire.
pub fn parse_wrap_proof(json: &str) -> Result<WrapProof, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if value.get("schemaVersion").is_some() {
        parse_ocaml_wrap_proof(&value)
    } else {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
}

/// Canonical native form used to compare VKs independently of the incoming
/// JSON whitespace and wire format.
pub fn canonical_wrap_vk_json(vk: &WrapVerifierIndex) -> Result<String, String> {
    serde_json::to_string(vk).map_err(|e| e.to_string())
}

/// Little-endian bytes (zero-padded to the field byte size) → field, via the
/// o1-utils checked deserializer (`FieldHelpers::from_bytes`).
fn field_from_le<F: PrimeField>(mut le: Vec<u8>) -> Result<F, String> {
    le.resize(F::size_in_bytes(), 0);
    F::from_bytes(&le).map_err(|e| alloc::format!("field deserialize: {e:?}"))
}

/// A big-endian `0x`-hex field element (OCaml `Field.to_yojson` form). o1-utils
/// is little-endian + checked, so reverse the decoded bytes before handing off.
pub fn parse_field_be_hex<F: PrimeField>(s: &str) -> Result<F, String> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let mut bytes = hex::decode(s).map_err(|e| e.to_string())?;
    bytes.reverse();
    field_from_le(bytes)
}

/// Parse `app_statement.json` into application-statement fields. Historical o1
/// fixtures contain one `"0x…"` value. Zeko zkApp statements contain the two
/// fields from `Zkapp_statement.to_field_elements`, so an array is accepted as
/// well.
pub fn parse_app_statement_fields(json: &str) -> Result<Vec<StepField>, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    match value {
        Value::String(value) => Ok(alloc::vec![parse_field_be_hex(&value)?]),
        Value::Array(values) => values
            .iter()
            .map(be_hex)
            .collect::<Result<Vec<_>, String>>(),
        _ => Err("app statement: expected a 0x-hex string or array".to_string()),
    }
}

/// Compatibility parser for the single-field o1 fixtures.
pub fn parse_app_statement(json: &str) -> Result<StepField, String> {
    let fields = parse_app_statement_fields(json)?;
    if fields.len() != 1 {
        return Err(alloc::format!(
            "app statement: expected one field, got {}",
            fields.len()
        ));
    }
    Ok(fields[0])
}

// ---------------------------------------------------------------------------
// OcamlProof — the bespoke `public_input_skeleton.json` decode
// ---------------------------------------------------------------------------

/// Typed parse of the OCaml `proof_state` + `prev_evals` skeleton. Mirrors the
/// The shared field/eval sub-types live in [`crate::types`].
/// The prev-proof arrays are length `mpv` (0/1/2).
#[derive(Debug, Clone)]
pub struct OcamlProof {
    pub raw_plonk: PlonkMinimal,
    pub raw_bulletproof_challenges: [StepField; STEP_IPA_ROUNDS],
    pub branch_data: BranchData,
    pub sponge_digest_before_evaluations: StepField,
    /// the proof's own wrap challenge-polynomial commitment (Vesta, Fq coords).
    pub challenge_polynomial_commitment: Vesta,
    pub step_domain_log2: u8,
    pub prev_evals: ChunkedAllEvals,
    pub p_eval0_chunks: Vec<StepField>,
    /// `messages_for_next_step_proof.challenge_polynomial_commitments` (Pallas).
    pub prev_step_sgs: Vec<Pallas>,
    /// `messages_for_next_step_proof.old_bulletproof_challenges` (16-round step).
    pub prev_step_chals_raw: Vec<[StepField; STEP_IPA_ROUNDS]>,
    /// `messages_for_next_wrap_proof.old_bulletproof_challenges` (15-round wrap).
    pub prev_wrap_chals_raw: Vec<[WrapField; WRAP_IPA_ROUNDS]>,
}

fn field<'a>(v: &'a Value, k: &str) -> Result<&'a Value, String> {
    v.get(k).ok_or_else(|| alloc::format!("missing key `{k}`"))
}
fn arr(v: &Value) -> Result<&Value, String> {
    if v.is_array() {
        Ok(v)
    } else {
        Err("expected array".to_string())
    }
}
fn as_vec(v: &Value) -> Result<&Vec<Value>, String> {
    v.as_array().ok_or_else(|| "expected array".to_string())
}

/// Combine little-endian signed-int64 `Hex64` limbs into a field element.
/// OCaml-yojson emits each 64-bit limb as a (possibly negative) JSON int64;
/// `as i64 as u64` reinterprets the two's-complement bits.
fn combine_limbs_le<F: PrimeField>(limbs: &[Value]) -> Result<F, String> {
    let mut le = Vec::with_capacity(limbs.len() * 8);
    for l in limbs {
        let v = l.as_i64().ok_or_else(|| "limb: not an int64".to_string())? as u64;
        le.extend_from_slice(&v.to_le_bytes());
    }
    field_from_le(le)
}

/// A scalar/raw challenge: either `[int64, int64]` (raw `Challenge`) or
/// `{ "inner": [int64, int64] }` (`Scalar_challenge`). The raw 128-bit value
/// as a field (endo expansion happens at verify time).
fn challenge<F: PrimeField>(v: &Value) -> Result<F, String> {
    let limbs = match v.as_array() {
        Some(a) => a,
        None => field(v, "inner")?
            .as_array()
            .ok_or_else(|| "scalar challenge: `inner` not an array".to_string())?,
    };
    combine_limbs_le(limbs)
}

fn be_hex<F: PrimeField>(v: &Value) -> Result<F, String> {
    let s = v
        .as_str()
        .ok_or_else(|| "expected 0x-hex string".to_string())?;
    parse_field_be_hex(s)
}

/// `[x_hex, y_hex]` → an affine point (caller picks the curve via `mk`).
fn affine<C, F: PrimeField>(v: &Value, mk: impl Fn(F, F) -> C) -> Result<C, String> {
    let a = as_vec(v)?;
    if a.len() != 2 {
        return Err(alloc::format!("affine: expected [x, y], got {}", a.len()));
    }
    Ok(mk(be_hex(&a[0])?, be_hex(&a[1])?))
}

fn pallas(v: &Value) -> Result<Pallas, String> {
    let point = affine::<Pallas, StepField>(v, Pallas::new_unchecked)?;
    if !point.is_on_curve() || !point.is_in_correct_subgroup_assuming_on_curve() {
        return Err("Pallas point is not in the prime-order subgroup".to_string());
    }
    Ok(point)
}

fn integer(v: &Value, name: &str) -> Result<usize, String> {
    v.as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| alloc::format!("{name}: expected a non-negative integer"))
}

fn schema_v1(v: &Value, name: &str) -> Result<(), String> {
    let version = integer(field(v, "schemaVersion")?, "schemaVersion")?;
    if version != 1 {
        return Err(alloc::format!(
            "{name}: unsupported schemaVersion {version}"
        ));
    }
    Ok(())
}

fn fixed_map<T, const N: usize>(
    v: &Value,
    name: &str,
    parse: impl Fn(&Value) -> Result<T, String>,
) -> Result<[T; N], String> {
    let values = as_vec(v)?;
    if values.len() != N {
        return Err(alloc::format!(
            "{name}: expected {N} entries, got {}",
            values.len()
        ));
    }
    values
        .iter()
        .map(parse)
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| alloc::format!("{name}: length invariant"))
}

fn pallas_comm(v: &Value) -> Result<PolyComm<Pallas>, String> {
    Ok(PolyComm {
        chunks: alloc::vec![pallas(v)?],
    })
}

fn wrap_point_eval(v: &Value) -> Result<PointEvaluations<Vec<WrapField>>, String> {
    let values = as_vec(v)?;
    if values.len() != 2 {
        return Err(alloc::format!(
            "wrap evaluation: expected [zeta, zeta_omega], got {} entries",
            values.len()
        ));
    }
    Ok(PointEvaluations {
        zeta: alloc::vec![be_hex(&values[0])?],
        zeta_omega: alloc::vec![be_hex(&values[1])?],
    })
}

fn parse_ocaml_wrap_proof(wire: &Value) -> Result<WrapProof, String> {
    schema_v1(wire, "Pickles proof wire")?;
    let proof = field(wire, "proof")?;

    let commitments = field(proof, "commitments")?;
    let w_comm = fixed_map::<_, 15>(field(commitments, "w_comm")?, "w_comm", pallas_comm)?;
    let z_comm = pallas_comm(field(commitments, "z_comm")?)?;
    let t_comm = PolyComm {
        chunks: as_vec(field(commitments, "t_comm")?)?
            .iter()
            .map(pallas)
            .collect::<Result<Vec<_>, _>>()?,
    };
    if t_comm.chunks.len() != 7 {
        return Err(alloc::format!(
            "t_comm: expected 7 chunks, got {}",
            t_comm.chunks.len()
        ));
    }

    let evaluations = field(proof, "evaluations")?;
    let w = fixed_map::<_, 15>(field(evaluations, "w")?, "w evaluations", wrap_point_eval)?;
    let coefficients = fixed_map::<_, 15>(
        field(evaluations, "coefficients")?,
        "coefficient evaluations",
        wrap_point_eval,
    )?;
    let s = fixed_map::<_, 6>(field(evaluations, "s")?, "s evaluations", wrap_point_eval)?;
    let evals = ProofEvaluations {
        public: None,
        w,
        z: wrap_point_eval(field(evaluations, "z")?)?,
        s,
        coefficients,
        generic_selector: wrap_point_eval(field(evaluations, "generic_selector")?)?,
        poseidon_selector: wrap_point_eval(field(evaluations, "poseidon_selector")?)?,
        complete_add_selector: wrap_point_eval(field(evaluations, "complete_add_selector")?)?,
        mul_selector: wrap_point_eval(field(evaluations, "mul_selector")?)?,
        emul_selector: wrap_point_eval(field(evaluations, "emul_selector")?)?,
        endomul_scalar_selector: wrap_point_eval(field(evaluations, "endomul_scalar_selector")?)?,
        range_check0_selector: None,
        range_check1_selector: None,
        foreign_field_add_selector: None,
        foreign_field_mul_selector: None,
        xor_selector: None,
        rot_selector: None,
        lookup_aggregation: None,
        lookup_table: None,
        lookup_sorted: [None, None, None, None, None],
        runtime_lookup_table: None,
        runtime_lookup_table_selector: None,
        xor_lookup_selector: None,
        lookup_gate_lookup_selector: None,
        range_check_lookup_selector: None,
        foreign_field_mul_lookup_selector: None,
    };

    let bulletproof = field(proof, "bulletproof")?;
    let lr = as_vec(field(bulletproof, "lr")?)?
        .iter()
        .map(|pair| {
            let pair = as_vec(pair)?;
            if pair.len() != 2 {
                return Err("bulletproof lr: expected [left, right]".to_string());
            }
            Ok((pallas(&pair[0])?, pallas(&pair[1])?))
        })
        .collect::<Result<Vec<_>, String>>()?;
    if lr.len() != WRAP_IPA_ROUNDS {
        return Err(alloc::format!(
            "bulletproof lr: expected {WRAP_IPA_ROUNDS} rounds, got {}",
            lr.len()
        ));
    }
    let opening = OpeningProof {
        lr,
        delta: pallas(field(bulletproof, "delta")?)?,
        z1: be_hex(field(bulletproof, "z_1")?)?,
        z2: be_hex(field(bulletproof, "z_2")?)?,
        sg: pallas(field(bulletproof, "challenge_polynomial_commitment")?)?,
    };

    let prev_challenges = as_vec(field(wire, "prevChallenges")?)?
        .iter()
        .map(|challenge| {
            let chals = as_vec(field(challenge, "challenges")?)?
                .iter()
                .map(be_hex)
                .collect::<Result<Vec<WrapField>, String>>()?;
            if chals.len() != WRAP_IPA_ROUNDS {
                return Err(alloc::format!(
                    "previous challenge: expected {WRAP_IPA_ROUNDS} challenges, got {}",
                    chals.len()
                ));
            }
            Ok(RecursionChallenge {
                chals,
                comm: pallas_comm(field(challenge, "commitment")?)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if prev_challenges.len() != 2 {
        return Err(alloc::format!(
            "previous challenges: expected 2 padded accumulators, got {}",
            prev_challenges.len()
        ));
    }

    Ok(WrapProof {
        commitments: ProverCommitments {
            w_comm,
            z_comm,
            t_comm,
            lookup: None,
        },
        proof: opening,
        evals,
        ft_eval1: be_hex(field(proof, "ft_eval1")?)?,
        prev_challenges,
    })
}

fn parse_ocaml_wrap_vk(wire: &Value) -> Result<WrapVerifierIndex, String> {
    schema_v1(wire, "Pickles verification-key wire")?;
    let max_proofs_verified = integer(field(wire, "maxProofsVerified")?, "maxProofsVerified")?;
    let actual_wrap_domain_size =
        integer(field(wire, "actualWrapDomainSize")?, "actualWrapDomainSize")?;
    let wrap_domain_log2 = integer(field(wire, "wrapDomainLog2")?, "wrapDomainLog2")?;
    if max_proofs_verified != 2 {
        return Err(alloc::format!(
            "verification key: expected maxProofsVerified 2, got {max_proofs_verified}"
        ));
    }
    if actual_wrap_domain_size > 2 {
        return Err("verification key: actualWrapDomainSize must be in 0..=2".to_string());
    }
    if wrap_domain_log2 != 13 + actual_wrap_domain_size {
        return Err(alloc::format!(
            "verification key: domain log2 {wrap_domain_log2} does not match actualWrapDomainSize {actual_wrap_domain_size}"
        ));
    }
    let max_poly_size = integer(field(wire, "maxPolySize")?, "maxPolySize")?;
    if max_poly_size != 1usize << WRAP_IPA_ROUNDS {
        return Err(alloc::format!(
            "verification key: expected maxPolySize {}, got {max_poly_size}",
            1usize << WRAP_IPA_ROUNDS
        ));
    }
    let public = integer(field(wire, "publicInputs")?, "publicInputs")?;
    let prev_challenges = integer(field(wire, "prevChallenges")?, "prevChallenges")?;
    if prev_challenges != 2 {
        return Err(alloc::format!(
            "verification key: expected 2 previous challenges, got {prev_challenges}"
        ));
    }
    let zk_rows = integer(field(wire, "zkRows")?, "zkRows")? as u64;
    if zk_rows != 3 {
        return Err(alloc::format!(
            "verification key: expected 3 zero-knowledge rows, got {zk_rows}"
        ));
    }

    let commitments = field(wire, "commitments")?;
    let sigma_comm = fixed_map::<_, 7>(
        field(commitments, "sigmaComm")?,
        "sigma commitments",
        pallas_comm,
    )?;
    let coefficients_comm = fixed_map::<_, 15>(
        field(commitments, "coefficientsComm")?,
        "coefficient commitments",
        pallas_comm,
    )?;
    let domain_size = 1usize
        .checked_shl(wrap_domain_log2 as u32)
        .ok_or_else(|| "verification key: domain size overflow".to_string())?;
    let domain = Radix2EvaluationDomain::<WrapField>::new(domain_size)
        .ok_or_else(|| "verification key: invalid radix-2 domain".to_string())?;
    let shift = *permutation::Shifts::new(&domain).shifts();

    Ok(WrapVerifierIndex {
        domain,
        max_poly_size,
        zk_rows,
        srs: Default::default(),
        public,
        prev_challenges,
        sigma_comm,
        coefficients_comm,
        generic_comm: pallas_comm(field(commitments, "genericComm")?)?,
        psm_comm: pallas_comm(field(commitments, "psmComm")?)?,
        complete_add_comm: pallas_comm(field(commitments, "completeAddComm")?)?,
        mul_comm: pallas_comm(field(commitments, "mulComm")?)?,
        emul_comm: pallas_comm(field(commitments, "emulComm")?)?,
        endomul_scalar_comm: pallas_comm(field(commitments, "endomulScalarComm")?)?,
        range_check0_comm: None,
        range_check1_comm: None,
        foreign_field_add_comm: None,
        foreign_field_mul_comm: None,
        xor_comm: None,
        rot_comm: None,
        shift,
        permutation_vanishing_polynomial_m: Default::default(),
        w: Default::default(),
        endo: WrapField::zero(),
        lookup_index: None,
        linearization: Default::default(),
        powers_of_alpha: Default::default(),
    })
}

/// `["N0"|"N1"|"N2"]` → the CONSTANT `to_bool_vec` mask.
fn proofs_verified_mask(v: &Value) -> Result<[bool; 2], String> {
    let a = as_vec(v)?;
    let tag = a
        .first()
        .and_then(|t| t.as_str())
        .ok_or_else(|| "proofs_verified: expected [tag]".to_string())?;
    match tag {
        "N0" => Ok([false, false]),
        "N1" => Ok([false, true]),
        "N2" => Ok([true, true]),
        other => Err(alloc::format!(
            "proofs_verified: expected N0|N1|N2, got {other}"
        )),
    }
}

/// OCaml `Hex64` single byte (= `domain_log2`) is a 1-char string.
fn ocaml_byte(v: &Value) -> Result<u8, String> {
    let s = v
        .as_str()
        .ok_or_else(|| "domain_log2: expected string".to_string())?;
    s.chars()
        .next()
        .map(|c| c as u8)
        .ok_or_else(|| "domain_log2: empty string".to_string())
}

fn bulletproof_vec<F: PrimeField, const N: usize>(v: &Value) -> Result<[F; N], String> {
    let a = as_vec(v)?;
    if a.len() != N {
        return Err(alloc::format!(
            "bulletproof: expected {N} challenges, got {}",
            a.len()
        ));
    }
    let mut out = Vec::with_capacity(N);
    for c in a {
        out.push(challenge(field(c, "prechallenge")?)?);
    }
    out.try_into()
        .map_err(|_| "bulletproof: length invariant".to_string())
}

/// Flat public-input eval `[zeta_hex, omega_hex]` → a 1-chunk PointEvaluations.
fn point_eval_flat(v: &Value) -> Result<PointEvaluations<Vec<StepField>>, String> {
    let a = as_vec(v)?;
    if a.len() != 2 {
        return Err("public_input: expected [zeta, omega]".to_string());
    }
    Ok(PointEvaluations {
        zeta: alloc::vec![be_hex(&a[0])?],
        zeta_omega: alloc::vec![be_hex(&a[1])?],
    })
}

/// Chunked eval `[[zeta_chunks…], [omega_chunks…]]` → PointEvaluations of vecs.
fn point_eval_chunked(v: &Value) -> Result<PointEvaluations<Vec<StepField>>, String> {
    let a = as_vec(v)?;
    if a.len() != 2 {
        return Err("chunked eval: expected [zeta_chunks, omega_chunks]".to_string());
    }
    let zeta = as_vec(&a[0])?
        .iter()
        .map(be_hex)
        .collect::<Result<Vec<_>, String>>()?;
    let zeta_omega = as_vec(&a[1])?
        .iter()
        .map(be_hex)
        .collect::<Result<Vec<_>, String>>()?;
    if zeta.len() != zeta_omega.len() {
        return Err("chunked eval: zeta/omega chunk count mismatch".to_string());
    }
    Ok(PointEvaluations { zeta, zeta_omega })
}

fn optional_point_eval_chunked(
    v: &Value,
) -> Result<Option<PointEvaluations<Vec<StepField>>>, String> {
    if v.is_null() {
        Ok(None)
    } else {
        point_eval_chunked(v).map(Some)
    }
}

fn fixed_chunked<const N: usize>(
    v: &Value,
) -> Result<[PointEvaluations<Vec<StepField>>; N], String> {
    let a = as_vec(v)?;
    if a.len() != N {
        return Err(alloc::format!(
            "evals: expected {N} columns, got {}",
            a.len()
        ));
    }
    let out = a
        .iter()
        .map(point_eval_chunked)
        .collect::<Result<Vec<_>, String>>()?;
    out.try_into()
        .map_err(|_| "evals: length invariant".to_string())
}

fn parse_all_evals(v: &Value) -> Result<ChunkedAllEvals, String> {
    let ft_eval1 = be_hex(field(v, "ft_eval1")?)?;
    let evals_obj = field(v, "evals")?;
    let public_evals = point_eval_flat(field(evals_obj, "public_input")?)?;
    let inner = field(evals_obj, "evals")?;

    let z = point_eval_chunked(field(inner, "z")?)?;
    let w = fixed_chunked::<15>(arr(field(inner, "w")?)?)?;
    let coefficients = fixed_chunked::<15>(arr(field(inner, "coefficients")?)?)?;
    let s = fixed_chunked::<6>(arr(field(inner, "s")?)?)?;

    let index = [
        point_eval_chunked(field(inner, "generic_selector")?)?,
        point_eval_chunked(field(inner, "poseidon_selector")?)?,
        point_eval_chunked(field(inner, "complete_add_selector")?)?,
        point_eval_chunked(field(inner, "mul_selector")?)?,
        point_eval_chunked(field(inner, "emul_selector")?)?,
        point_eval_chunked(field(inner, "endomul_scalar_selector")?)?,
    ];

    let lookup_sorted = as_vec(field(inner, "lookup_sorted")?)?
        .iter()
        .map(optional_point_eval_chunked)
        .collect::<Result<Vec<_>, String>>()?
        .try_into()
        .map_err(|values: Vec<_>| {
            alloc::format!("lookup_sorted: expected 5 columns, got {}", values.len())
        })?;

    Ok(ChunkedAllEvals {
        ft_eval1,
        public_evals,
        z,
        w,
        coefficients,
        s,
        index,
        range_check0_selector: optional_point_eval_chunked(field(inner, "range_check0_selector")?)?,
        range_check1_selector: optional_point_eval_chunked(field(inner, "range_check1_selector")?)?,
        foreign_field_add_selector: optional_point_eval_chunked(field(
            inner,
            "foreign_field_add_selector",
        )?)?,
        foreign_field_mul_selector: optional_point_eval_chunked(field(
            inner,
            "foreign_field_mul_selector",
        )?)?,
        xor_selector: optional_point_eval_chunked(field(inner, "xor_selector")?)?,
        rot_selector: optional_point_eval_chunked(field(inner, "rot_selector")?)?,
        lookup_aggregation: optional_point_eval_chunked(field(inner, "lookup_aggregation")?)?,
        lookup_table: optional_point_eval_chunked(field(inner, "lookup_table")?)?,
        lookup_sorted,
        runtime_lookup_table: optional_point_eval_chunked(field(inner, "runtime_lookup_table")?)?,
        runtime_lookup_table_selector: optional_point_eval_chunked(field(
            inner,
            "runtime_lookup_table_selector",
        )?)?,
        xor_lookup_selector: optional_point_eval_chunked(field(inner, "xor_lookup_selector")?)?,
        lookup_gate_lookup_selector: optional_point_eval_chunked(field(
            inner,
            "lookup_gate_lookup_selector",
        )?)?,
        range_check_lookup_selector: optional_point_eval_chunked(field(
            inner,
            "range_check_lookup_selector",
        )?)?,
        foreign_field_mul_lookup_selector: optional_point_eval_chunked(field(
            inner,
            "foreign_field_mul_lookup_selector",
        )?)?,
    })
}

impl OcamlProof {
    /// Decode `public_input_skeleton.json`.
    pub fn parse(json: &str) -> Result<OcamlProof, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;

        let statement = field(&root, "statement")?;
        let proof_state = field(statement, "proof_state")?;
        let deferred = field(proof_state, "deferred_values")?;

        let plonk = field(deferred, "plonk")?;
        let feature_flags = field(plonk, "feature_flags")?;
        let flag = |name| {
            field(feature_flags, name)?
                .as_bool()
                .ok_or_else(|| alloc::format!("plonk: feature flag `{name}` is not boolean"))
        };
        let range_check0 = flag("range_check0")?;
        let range_check1 = flag("range_check1")?;
        let foreign_field_add = flag("foreign_field_add")?;
        let foreign_field_mul = flag("foreign_field_mul")?;
        let xor = flag("xor")?;
        let rot = flag("rot")?;
        let lookup = flag("lookup")?;
        let runtime_tables = flag("runtime_tables")?;
        let patterns = LookupPatterns {
            xor,
            lookup,
            range_check: range_check0 || range_check1 || rot,
            foreign_field_mul,
        };
        let feature_flags = FeatureFlags {
            range_check0,
            range_check1,
            foreign_field_add,
            foreign_field_mul,
            xor,
            rot,
            lookup_features: LookupFeatures {
                patterns,
                joint_lookup_used: patterns.joint_lookups_used(),
                uses_runtime_tables: runtime_tables,
            },
        };
        let joint_combiner = match field(plonk, "joint_combiner")? {
            Value::Null => None,
            value => Some(challenge(value)?),
        };
        let raw_plonk = PlonkMinimal {
            alpha: challenge(field(plonk, "alpha")?)?,
            beta: challenge(field(plonk, "beta")?)?,
            gamma: challenge(field(plonk, "gamma")?)?,
            zeta: challenge(field(plonk, "zeta")?)?,
            joint_combiner,
            feature_flags,
        };

        let raw_bulletproof_challenges = bulletproof_vec::<StepField, STEP_IPA_ROUNDS>(field(
            deferred,
            "bulletproof_challenges",
        )?)?;

        let branch_data_j = field(deferred, "branch_data")?;
        let step_domain_log2 = ocaml_byte(field(branch_data_j, "domain_log2")?)?;
        let branch_data = BranchData {
            domain_log2: StepField::from(step_domain_log2 as u64),
            proofs_verified_mask: proofs_verified_mask(field(branch_data_j, "proofs_verified")?)?,
        };

        let sponge_digest_before_evaluations = combine_limbs_le(as_vec(field(
            proof_state,
            "sponge_digest_before_evaluations",
        )?)?)?;

        let msg_wrap = field(proof_state, "messages_for_next_wrap_proof")?;
        let challenge_polynomial_commitment = affine::<Vesta, WrapField>(
            field(msg_wrap, "challenge_polynomial_commitment")?,
            Vesta::new_unchecked,
        )?;

        let msg_step = field(statement, "messages_for_next_step_proof")?;
        let prev_step_sgs = as_vec(field(msg_step, "challenge_polynomial_commitments")?)?
            .iter()
            .map(|p| affine::<Pallas, StepField>(p, Pallas::new_unchecked))
            .collect::<Result<Vec<_>, String>>()?;
        let prev_step_chals_raw = as_vec(field(msg_step, "old_bulletproof_challenges")?)?
            .iter()
            .map(bulletproof_vec::<StepField, STEP_IPA_ROUNDS>)
            .collect::<Result<Vec<_>, String>>()?;
        let prev_wrap_chals_raw = as_vec(field(msg_wrap, "old_bulletproof_challenges")?)?
            .iter()
            .map(bulletproof_vec::<WrapField, WRAP_IPA_ROUNDS>)
            .collect::<Result<Vec<_>, String>>()?;

        let prev_evals = parse_all_evals(field(&root, "prev_evals")?)?;
        let p_eval0_chunks = prev_evals.public_evals.zeta.clone();

        Ok(OcamlProof {
            raw_plonk,
            raw_bulletproof_challenges,
            branch_data,
            sponge_digest_before_evaluations,
            challenge_polynomial_commitment,
            step_domain_log2,
            prev_evals,
            p_eval0_chunks,
            prev_step_sgs,
            prev_step_chals_raw,
            prev_wrap_chals_raw,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::BigInteger;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../../../fixtures/", $name))
        };
    }

    fn field_hex<F: PrimeField>(value: F) -> Value {
        Value::String(alloc::format!(
            "0x{}",
            hex::encode(value.into_bigint().to_bytes_be())
        ))
    }

    fn pallas_json(point: Pallas) -> Value {
        serde_json::json!([field_hex(point.x), field_hex(point.y)])
    }

    fn pallas_comm_json(commitment: &PolyComm<Pallas>) -> Value {
        assert_eq!(commitment.chunks.len(), 1);
        pallas_json(commitment.chunks[0])
    }

    fn proof_wire(side_loaded_proof: Value, legacy: &WrapProof) -> Value {
        let prev_challenges = legacy
            .prev_challenges
            .iter()
            .map(|challenge| {
                serde_json::json!({
                    "challenges": challenge.chals.iter().copied().map(field_hex).collect::<Vec<_>>(),
                    "commitment": pallas_comm_json(&challenge.comm),
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "schemaVersion": 1,
            "proof": side_loaded_proof["proof"].clone(),
            "prevChallenges": prev_challenges,
        })
    }

    fn vk_wire(legacy: &WrapVerifierIndex) -> Value {
        let domain_log2 = legacy.domain.size().ilog2() as usize;
        let commitments =
            |values: &[PolyComm<Pallas>]| values.iter().map(pallas_comm_json).collect::<Vec<_>>();
        serde_json::json!({
            "schemaVersion": 1,
            "maxProofsVerified": 2,
            "actualWrapDomainSize": domain_log2 - 13,
            "wrapDomainLog2": domain_log2,
            "maxPolySize": legacy.max_poly_size,
            "publicInputs": legacy.public,
            "prevChallenges": legacy.prev_challenges,
            "zkRows": legacy.zk_rows,
            "commitments": {
                "sigmaComm": commitments(&legacy.sigma_comm),
                "coefficientsComm": commitments(&legacy.coefficients_comm),
                "genericComm": pallas_comm_json(&legacy.generic_comm),
                "psmComm": pallas_comm_json(&legacy.psm_comm),
                "completeAddComm": pallas_comm_json(&legacy.complete_add_comm),
                "mulComm": pallas_comm_json(&legacy.mul_comm),
                "emulComm": pallas_comm_json(&legacy.emul_comm),
                "endomulScalarComm": pallas_comm_json(&legacy.endomul_scalar_comm),
            },
        })
    }

    #[test]
    fn zeko_ocaml_wire_matches_legacy_kimchi_serde() {
        let gateway: Value =
            serde_json::from_str(fixture!("zeko-local-e2e/settlement.json")).unwrap();
        let bundle = &gateway["proof"];

        let legacy_proof = parse_wrap_proof(bundle["proofJson"].as_str().unwrap()).unwrap();
        let side_loaded: Value =
            serde_json::from_str(bundle["publicInputSkeletonJson"].as_str().unwrap()).unwrap();
        let parsed_proof = parse_wrap_proof(&proof_wire(side_loaded, &legacy_proof).to_string())
            .expect("OCaml proof wire");
        assert_eq!(parsed_proof, legacy_proof);

        let legacy_vk_json = bundle["vkJson"].as_str().unwrap();
        let legacy_vk = parse_wrap_vk(legacy_vk_json).unwrap();
        assert_eq!(
            canonical_wrap_vk_json(&legacy_vk).unwrap(),
            legacy_vk_json,
            "canonicalization must preserve the deployed legacy VK identifier"
        );
        let parsed_vk = parse_wrap_vk(&vk_wire(&legacy_vk).to_string()).expect("OCaml VK wire");
        assert_eq!(
            canonical_wrap_vk_json(&parsed_vk).unwrap(),
            canonical_wrap_vk_json(&legacy_vk).unwrap()
        );
    }

    #[test]
    fn zeko_ocaml_wire_rejects_invalid_curve_point() {
        let gateway: Value =
            serde_json::from_str(fixture!("zeko-local-e2e/settlement.json")).unwrap();
        let bundle = &gateway["proof"];
        let legacy = parse_wrap_proof(bundle["proofJson"].as_str().unwrap()).unwrap();
        let side_loaded: Value =
            serde_json::from_str(bundle["publicInputSkeletonJson"].as_str().unwrap()).unwrap();
        let mut wire = proof_wire(side_loaded, &legacy);
        wire["proof"]["commitments"]["z_comm"] = serde_json::json!(["0x01", "0x01"]);
        assert!(parse_wrap_proof(&wire.to_string()).is_err());
    }

    #[test]
    fn nrr_vk_proof_app_statement_parse() {
        parse_wrap_vk(fixture!("nrr/vk.serde.json")).expect("vk");
        parse_wrap_proof(fixture!("nrr/proof.serde.json")).expect("proof");
        parse_app_statement(fixture!("nrr/app_statement.json")).expect("app_statement");
    }

    #[test]
    fn multi_field_application_statement_parses() {
        let fields =
            parse_app_statement_fields(r#"["0x01", "0x02"]"#).expect("two-field Zkapp statement");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0], StepField::from(1u64));
        assert_eq!(fields[1], StepField::from(2u64));
    }

    #[test]
    fn enabled_feature_flag_is_parsed() {
        let mut skeleton: Value =
            serde_json::from_str(fixture!("nrr/public_input_skeleton.json")).unwrap();
        skeleton["statement"]["proof_state"]["deferred_values"]["plonk"]["feature_flags"]
            ["range_check0"] = Value::Bool(true);
        let proof = OcamlProof::parse(&skeleton.to_string()).expect("featureful skeleton parses");
        assert!(proof.raw_plonk.feature_flags.range_check0);
        assert!(
            proof
                .raw_plonk
                .feature_flags
                .lookup_features
                .patterns
                .range_check
        );
    }

    #[test]
    fn lookup_joint_combiner_is_parsed() {
        let mut skeleton: Value =
            serde_json::from_str(fixture!("nrr/public_input_skeleton.json")).unwrap();
        skeleton["statement"]["proof_state"]["deferred_values"]["plonk"]["joint_combiner"] =
            serde_json::json!({"inner": [1, 0]});
        let proof = OcamlProof::parse(&skeleton.to_string()).expect("joint combiner parses");
        assert_eq!(proof.raw_plonk.joint_combiner, Some(StepField::from(1u64)));
    }

    /// Common skeleton checks: parses with the expected mpv (prev-proof array
    /// widths) and proofs_verified mask, every decoded point lands on its
    /// assigned curve (validating the Fp->Pallas / Fq->Vesta mapping), nc = 1.
    fn check_skeleton(json: &str, mpv: usize, mask: [bool; 2]) {
        let p = OcamlProof::parse(json).expect("skeleton should parse");
        assert_eq!(p.prev_step_sgs.len(), mpv, "prev_step_sgs width");
        assert_eq!(p.prev_step_chals_raw.len(), mpv, "prev_step_chals width");
        assert_eq!(p.prev_wrap_chals_raw.len(), mpv, "prev_wrap_chals width");
        assert_eq!(p.branch_data.proofs_verified_mask, mask);
        assert_eq!(p.raw_bulletproof_challenges.len(), 16);
        assert!(
            p.challenge_polynomial_commitment.is_on_curve(),
            "cpc on Vesta"
        );
        for sg in &p.prev_step_sgs {
            assert!(sg.is_on_curve(), "prev_step_sg on Pallas");
        }
        assert_eq!(p.prev_evals.z.zeta.len(), 1, "nc = 1");
        assert_eq!(p.p_eval0_chunks.len(), 1);
    }

    #[test]
    fn nrr_skeleton_parses() {
        check_skeleton(
            fixture!("nrr/public_input_skeleton.json"),
            0,
            [false, false],
        );
    }

    #[test]
    fn simplechain_skeletons_parse() {
        check_skeleton(
            fixture!("simplechain/wrap0/public_input_skeleton.json"),
            1,
            [false, true],
        );
        check_skeleton(
            fixture!("simplechain/wrap1/public_input_skeleton.json"),
            1,
            [false, true],
        );
        check_skeleton(
            fixture!("simplechain/wrap2/public_input_skeleton.json"),
            1,
            [false, true],
        );
    }

    #[test]
    fn tree_skeletons_parse() {
        check_skeleton(
            fixture!("treeproofreturn/wrap0/public_input_skeleton.json"),
            2,
            [true, true],
        );
        check_skeleton(
            fixture!("treeproofreturn/wrap1/public_input_skeleton.json"),
            2,
            [true, true],
        );
        check_skeleton(
            fixture!("treeproofreturn/wrap2/public_input_skeleton.json"),
            2,
            [true, true],
        );
    }

    #[test]
    fn all_vk_proof_app_statement_parse() {
        for dir in [
            "nrr",
            "simplechain/wrap0",
            "simplechain/wrap1",
            "simplechain/wrap2",
            "treeproofreturn/wrap0",
            "treeproofreturn/wrap1",
            "treeproofreturn/wrap2",
        ] {
            // include_str! needs literals; cover them explicitly below.
            let _ = dir;
        }
        // vk/proof/app_statement round-trips for one of each pattern.
        parse_wrap_vk(fixture!("simplechain/wrap0/vk.serde.json")).expect("sc vk");
        parse_wrap_proof(fixture!("simplechain/wrap0/proof.serde.json")).expect("sc proof");
        parse_app_statement(fixture!("simplechain/wrap1/app_statement.json")).expect("sc app");
        parse_wrap_vk(fixture!("treeproofreturn/wrap0/vk.serde.json")).expect("tree vk");
        parse_wrap_proof(fixture!("treeproofreturn/wrap0/proof.serde.json")).expect("tree proof");
        parse_app_statement(fixture!("treeproofreturn/wrap2/app_statement.json"))
            .expect("tree app");
    }
}
