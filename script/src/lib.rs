use anyhow::{Context, Result};
use pickles_verifier::serialize::decode_verifier_blob;
use pickles_verifier::types::{VerifiableProof, Verifier};
use pickles_verifier::verify;
use pickles_verifier::wire::{
    canonical_wrap_vk_json, parse_app_statement_fields, parse_wrap_proof, parse_wrap_vk, OcamlProof,
};
use serde::{Deserialize, Serialize};
use sp1_core_executor::Program;
use sp1_core_executor_runner::MinimalExecutorRunner;
use sp1_sdk::{include_elf, Elf, SP1Stdin};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use zeko_sp1_lib::{
    AssetRegistryBatchCheckpointV4, AssetRegistryCheckpointV3, InnerActionBatchWitnessV2,
    SettlementBindingV1, SettlementContextV1, SettlementWitnessV1,
};

pub const SETTLEMENT_ELF: Elf = include_elf!("settlement-program");
pub const BRIDGE_ELF: Elf = include_elf!("bridge-program");
pub const WITHDRAW_ELF: Elf = include_elf!("withdraw-program");

#[repr(C, align(8))]
struct Aligned<T: ?Sized>(T);

static NATIVE_SETTLEMENT_VERIFIER_BYTES: &Aligned<[u8]> = &Aligned(*include_bytes!(concat!(
    env!("OUT_DIR"),
    "/native_settlement_verifier.bin"
)));
static NATIVE_SETTLEMENT_VK_HASH: &[u8; 32] =
    include_bytes!(concat!(env!("OUT_DIR"), "/native_settlement_vk_hash.bin"));
static NATIVE_SETTLEMENT_VK_JSON: &str =
    include_str!(concat!(env!("OUT_DIR"), "/native_settlement_vk.json"));
static NATIVE_SETTLEMENT_VERIFIER: OnceLock<Verifier> = OnceLock::new();
static NATIVE_SETTLEMENT_VK_CANONICAL: OnceLock<String> = OnceLock::new();

/// Execute without proving using the low-memory runner. SP1 SDK 6.1's execute
/// wrapper can fail while extracting the public-value digest for this very
/// large program even after successful execution.
pub fn execute_minimal(elf: Elf, stdin: SP1Stdin) -> Result<(Vec<u8>, u64)> {
    let program = Arc::new(
        Program::from(&*elf).map_err(|error| anyhow::anyhow!("disassemble program: {error}"))?,
    );
    let mut executor = MinimalExecutorRunner::simple(program);
    for input in stdin.buffer {
        executor.with_input(&input);
    }
    while executor
        .try_execute_chunk()
        .map_err(|error| anyhow::anyhow!("execute chunk: {error}"))?
        .is_some()
    {}
    let exit_code = executor.exit_code();
    if exit_code != 0 {
        anyhow::bail!("program exited with status {exit_code}");
    }
    let cycles = executor.global_clk();
    Ok((executor.into_public_values_stream(), cycles))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementProofBundle {
    pub vk_json: String,
    pub proof_json: String,
    pub public_input_skeleton_json: String,
    pub app_statement_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<SettlementBindingV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<SettlementContextV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_action_batch: Option<InnerActionBatchWitnessV2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_registry_checkpoint: Option<AssetRegistryCheckpointV3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_registry_batch: Option<AssetRegistryBatchCheckpointV4>,
}

pub fn settlement_stdin(fixture_dir: &str) -> Result<SP1Stdin> {
    let verifiable = load_verifiable(Path::new(fixture_dir))?;
    Ok(settlement_stdin_for_verifiable(&verifiable, None))
}

pub fn settlement_stdin_from_bundle(bundle: &SettlementProofBundle) -> Result<SP1Stdin> {
    let verifiable = load_verifiable_bundle(bundle)?;
    let witness = settlement_witness(bundle)?;
    Ok(settlement_stdin_for_verifiable(&verifiable, witness))
}

pub fn native_settlement_preflight(bundle: &SettlementProofBundle) -> Result<Vec<u8>> {
    let verifiable = native_verify_settlement(bundle)?;
    let witness = settlement_witness(bundle)?
        .context("settlement binding and context are required for native preflight")?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        settlement_program::derive_receipt_bytes(&verifiable, witness, *NATIVE_SETTLEMENT_VK_HASH)
    }))
    .map_err(|_| anyhow::anyhow!("settlement receipt derivation failed"))
}

fn native_verify_settlement(bundle: &SettlementProofBundle) -> Result<VerifiableProof> {
    let supplied_vk = parse_wrap_vk(&bundle.vk_json)
        .map_err(anyhow::Error::msg)
        .context("parse settlement VK JSON")?;
    let supplied_vk = canonical_wrap_vk_json(&supplied_vk).map_err(anyhow::Error::msg)?;
    let pinned_vk = NATIVE_SETTLEMENT_VK_CANONICAL.get_or_init(|| {
        let vk = parse_wrap_vk(NATIVE_SETTLEMENT_VK_JSON)
            .expect("build-generated settlement VK JSON must parse");
        canonical_wrap_vk_json(&vk).expect("build-generated settlement VK must canonicalize")
    });
    anyhow::ensure!(
        supplied_vk == *pinned_vk,
        "settlement VK does not match the pinned verifier"
    );
    let verifiable = load_verifiable_bundle(bundle)?;
    let verifier = NATIVE_SETTLEMENT_VERIFIER
        .get_or_init(|| decode_verifier_blob(&NATIVE_SETTLEMENT_VERIFIER_BYTES.0));
    anyhow::ensure!(
        verify(verifier, &verifiable),
        "Pickles proof verification failed"
    );
    Ok(verifiable)
}

fn settlement_witness(bundle: &SettlementProofBundle) -> Result<Option<SettlementWitnessV1>> {
    match (&bundle.binding, &bundle.context) {
        (Some(binding), Some(context)) => Ok(Some(SettlementWitnessV1 {
            binding: binding.clone(),
            context: context.clone(),
            inner_action_batch: bundle.inner_action_batch.clone(),
            asset_registry_checkpoint: bundle.asset_registry_checkpoint.clone(),
            asset_registry_batch: bundle.asset_registry_batch.clone(),
        })),
        (None, None) => Ok(None),
        _ => anyhow::bail!("settlement binding and context must either both be present or absent"),
    }
}

fn settlement_stdin_for_verifiable(
    verifiable: &VerifiableProof,
    witness: Option<SettlementWitnessV1>,
) -> SP1Stdin {
    let mut stdin = SP1Stdin::new();
    stdin.write(verifiable);
    stdin.write(&witness);
    stdin
}

fn load_verifiable(fixture_dir: &Path) -> Result<VerifiableProof> {
    let read = |name: &str| -> Result<String> {
        let path = fixture_dir.join(name);
        std::fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))
    };

    let vk_json = read("vk.serde.json")?;
    let proof_json = read("proof.serde.json")?;
    let skeleton_json = read("public_input_skeleton.json")?;
    let app_stmt_json = read("app_statement.json")?;

    load_verifiable_bundle(&SettlementProofBundle {
        vk_json,
        proof_json,
        public_input_skeleton_json: skeleton_json,
        app_statement_json: app_stmt_json,
        binding: None,
        context: None,
        inner_action_batch: None,
        asset_registry_checkpoint: None,
        asset_registry_batch: None,
    })
}

pub fn load_verifiable_bundle(bundle: &SettlementProofBundle) -> Result<VerifiableProof> {
    let wrap_vk = parse_wrap_vk(&bundle.vk_json)
        .map_err(anyhow::Error::msg)
        .context("parse vk JSON")?;
    let wrap_proof = parse_wrap_proof(&bundle.proof_json)
        .map_err(anyhow::Error::msg)
        .context("parse proof JSON")?;
    let ocaml = OcamlProof::parse(&bundle.public_input_skeleton_json)
        .map_err(anyhow::Error::msg)
        .context("parse public input skeleton JSON")?;
    let app_statement = parse_app_statement_fields(&bundle.app_statement_json)
        .map_err(anyhow::Error::msg)
        .context("parse application statement JSON")?;

    ocaml
        .into_verifiable(wrap_proof, &wrap_vk, &app_statement)
        .map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned_fixture() -> SettlementProofBundle {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../proofs/mainnet-blockchain-snark");
        let read = |name: &str| {
            std::fs::read_to_string(fixture.join(name))
                .unwrap_or_else(|error| panic!("read {name}: {error}"))
        };
        SettlementProofBundle {
            vk_json: read("vk.serde.json"),
            proof_json: read("proof.serde.json"),
            public_input_skeleton_json: read("public_input_skeleton.json"),
            app_statement_json: read("app_statement.json"),
            binding: None,
            context: None,
            inner_action_batch: None,
            asset_registry_checkpoint: None,
            asset_registry_batch: None,
        }
    }

    #[test]
    fn native_verifier_accepts_pinned_fixture_and_json_whitespace() {
        let mut fixture = pinned_fixture();
        fixture.vk_json.push('\n');
        native_verify_settlement(&fixture).expect("native verification");
    }

    #[test]
    fn native_verifier_rejects_mutated_statement() {
        let mut fixture = pinned_fixture();
        let last_digit = fixture
            .app_statement_json
            .rfind('1')
            .expect("fixture statement contains a one");
        fixture
            .app_statement_json
            .replace_range(last_digit..=last_digit, "2");
        let error = match native_verify_settlement(&fixture) {
            Ok(_) => panic!("mutation must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("verification failed"));
    }

    #[test]
    fn native_verifier_rejects_mutated_verification_key() {
        let mut fixture = pinned_fixture();
        let mut vk: serde_json::Value = serde_json::from_str(&fixture.vk_json).unwrap();
        vk["generic_comm"] = vk["psm_comm"].clone();
        fixture.vk_json = serde_json::to_string(&vk).unwrap();
        assert!(native_verify_settlement(&fixture).is_err());
    }
}
