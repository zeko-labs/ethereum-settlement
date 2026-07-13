//! Wire types: the serde-deserializable form of the four OCaml-serialized
//! fixture files a `dump_*_fixtures` tool emits.
//!
//!   * `vk.serde.json` — kimchi `VerifierIndex` (Rust serde)
//!   * `proof.serde.json` — kimchi wrap `ProverProof` (Rust serde)
//!   * `public_input_skeleton.json` — the Pickles `{statement; prev_evals}` skeleton ([`OcamlProof`])
//!   * `app_statement.json` — the application statement (BE-hex field)
//!
//! The kimchi VK + proof reuse the upstream serde impls directly (same crate
//! that produced the JSON). The Pickles skeleton uses the bespoke OCaml-yojson
//! decode in [`OcamlProof::parse`]. Field/curve mapping:
//!
//!   * `StepField` = Tick = `Fp` (Vesta scalar / Pallas base)
//!   * `WrapField` = Tock = `Fq` (Pallas scalar / Vesta base)
//!   * the wrap proof + VK live over the `Pallas` curve.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ark_ff::PrimeField;
use kimchi::circuits::constraints::FeatureFlags;
use kimchi::circuits::lookup::lookups::{LookupFeatures, LookupPatterns};
use kimchi::proof::PointEvaluations;
use mina_curves::pasta::{Pallas, Vesta};
use o1_utils::FieldHelpers;
use serde_json::Value;

use crate::types::{
    BranchData, ChunkedAllEvals, PlonkMinimal, StepField, WrapField, WrapProof, WrapVerifierIndex,
    STEP_IPA_ROUNDS, WRAP_IPA_ROUNDS,
};

/// Parse `vk.serde.json` into the kimchi verifier index (SRS still empty).
pub fn parse_wrap_vk(json: &str) -> serde_json::Result<WrapVerifierIndex> {
    serde_json::from_str(json)
}

/// Parse `proof.serde.json` into the kimchi wrap proof.
pub fn parse_wrap_proof(json: &str) -> serde_json::Result<WrapProof> {
    serde_json::from_str(json)
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

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../../../fixtures/", $name))
        };
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
